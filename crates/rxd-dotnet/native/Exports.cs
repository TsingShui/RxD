using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Threading.Tasks;
using ICSharpCode.Decompiler;
using ICSharpCode.Decompiler.CSharp;
using ICSharpCode.Decompiler.CSharp.OutputVisitor;
using ICSharpCode.Decompiler.IL;
using ICSharpCode.Decompiler.IL.Transforms;
using ICSharpCode.Decompiler.Metadata;
using ICSharpCode.Decompiler.TypeSystem;

namespace Rxd.Dotnet.Native;

public static unsafe class Exports
{
    private const int MaxInputBytes = 64 * 1024 * 1024;
    internal const int MaxOutputBytes = 32 * 1024 * 1024;

    internal static string? InstalledReferenceDirectory;

    [UnmanagedCallersOnly(EntryPoint = "rxd_set_reference_directory", CallConvs = new[] { typeof(CallConvCdecl) })]
    public static int SetReferenceDirectory(byte* path, int length)
    {
        if (path == null || length <= 0 || length > 32768)
            return 0;
        try
        {
            InstalledReferenceDirectory = new UTF8Encoding(false, true).GetString(new ReadOnlySpan<byte>(path, length));
            return Path.IsPathFullyQualified(InstalledReferenceDirectory) ? 1 : 0;
        }
        catch { return 0; }
    }

    // Input is the caller's fixed module snapshot, not a mutable filesystem path.
    [UnmanagedCallersOnly(EntryPoint = "rxd_decompile_method", CallConvs = new[] { typeof(CallConvCdecl) })]
    public static byte* DecompileMethod(byte* module, int moduleLen, int metadataToken, int* outLen)
        => Decompile(module, moduleLen, metadataToken, false, outLen);

    [UnmanagedCallersOnly(EntryPoint = "rxd_decompile_class", CallConvs = new[] { typeof(CallConvCdecl) })]
    public static byte* DecompileClass(byte* module, int moduleLen, int metadataToken, int* outLen)
        => Decompile(module, moduleLen, metadataToken, true, outLen);

    [UnmanagedCallersOnly(EntryPoint = "rxd_decompile_assembly", CallConvs = new[] { typeof(CallConvCdecl) })]
    public static byte* DecompileAssembly(byte* module, int moduleLen, int* outLen)
        => Decompile(module, moduleLen, null, false, outLen);

    [UnmanagedCallersOnly(EntryPoint = "rxd_free", CallConvs = new[] { typeof(CallConvCdecl) })]
    public static void Free(byte* buffer) => NativeMemory.Free(buffer);

    private static byte* Decompile(byte* module, int moduleLen, int? token, bool classScope, int* outLen)
    {
        if (outLen == null)
            return null;
        *outLen = 0;
        try
        {
            if (module == null || moduleLen <= 0)
                return Failure("invalid_input", outLen);
            if (moduleLen > MaxInputBytes)
                return Failure("input_limit", outLen);
            // The PEFile never opens the synthetic name or executes analyzed code.
            using var stream = new MemoryStream(new ReadOnlySpan<byte>(module, moduleLen).ToArray(), writable: false);
            using var file = new PEFile("rxd-input.dll", stream);
            EntityHandle handle = default;
            var selectedMethods = new HashSet<int>();
            if (token.HasValue)
            {
                handle = MetadataTokens.EntityHandle(token.Value);
                int row = MetadataTokens.GetRowNumber(handle);
                if (classScope)
                {
                    if (handle.Kind != HandleKind.TypeDefinition || row <= 0 || row > file.Metadata.TypeDefinitions.Count)
                        return Failure("invalid_type_token", outLen);
                    var definition = file.Metadata.GetTypeDefinition((TypeDefinitionHandle)handle);
                    if (row == 1 && file.Metadata.GetString(definition.Name) == "<Module>")
                        return Failure("invalid_type_token", outLen);
                    SelectMethods(file.Metadata, (TypeDefinitionHandle)handle, selectedMethods, new HashSet<TypeDefinitionHandle>(), 0);
                }
                else
                {
                    if (handle.Kind != HandleKind.MethodDefinition || row <= 0 || row > file.Metadata.MethodDefinitions.Count)
                        return Failure("invalid_method_token", outLen);
                    if (file.Metadata.GetMethodDefinition((MethodDefinitionHandle)handle).RelativeVirtualAddress == 0)
                        return Failure("method_without_il", outLen);
                    selectedMethods.Add(token.Value);
                }
            }
            var settings = new DecompilerSettings(LanguageVersion.CSharp10_0) {
                ShowXmlDocumentation = false,
                UseDebugSymbols = false
            };
            using var resolver = new FrameworkResolver(file);
            var typeSystem = new DecompilerTypeSystem(file, resolver, settings);
            var decompiler = new CSharpDecompiler(typeSystem, settings);
            // Check both the original IL and transformed functions, including nested lambdas.
            decompiler.ILTransforms.Insert(0, new RejectInvalidIL());
            decompiler.ILTransforms.Add(new RejectInvalidIL());
            var tree = token.HasValue ? decompiler.Decompile(handle) : decompiler.DecompileWholeModuleAsSingleFile();
            foreach (var function in tree.DescendantsAndSelf.SelectMany(node => node.Annotations.OfType<ILFunction>()))
                RejectInvalidIL.Check(function);
            using var text = new BoundedCodeWriter();
            var writer = TokenWriter.CreateWriterThatSetsLocationsInAST(text);
            tree.AcceptVisitor(new CSharpOutputVisitor(writer, settings.CSharpFormattingOptions));
            string code = text.ToString();
            if (string.IsNullOrWhiteSpace(code))
                return Failure("empty_code", outLen);
            if (Encoding.UTF8.GetByteCount(code) > MaxOutputBytes)
                return Failure("output_limit", outLen);
            var mapping = new SortedSet<(int MethodToken, int Offset, int Line)>();
            if (token.HasValue)
            {
                foreach (var entry in decompiler.CreateSequencePoints(tree))
                {
                    // Generated state machines/closures have independent offset domains.
                    // ILSpy labels recovered async/iterator functions with the kickoff
                    // token, but their sequence points still belong to MoveNext's IL.
                    if (entry.Key.MoveNextMethod != null || entry.Key.Method == null || entry.Key.Method.MetadataToken.Kind != HandleKind.MethodDefinition || !selectedMethods.Contains(MetadataTokens.GetToken(entry.Key.Method.MetadataToken)) || entry.Key.Method.ParentModule != typeSystem.MainModule)
                        continue;
                    foreach (var point in entry.Value)
                        if (!point.IsHidden && point.Offset >= 0 && point.StartLine > 0)
                            mapping.Add((classScope ? MetadataTokens.GetToken(entry.Key.Method.MetadataToken) : 0, point.Offset, point.StartLine));
                }
            }
            return CopyOut(Payload(null, code, mapping), outLen);
        }
        catch (NativeFailure ex)
        {
            return Failure(ex.Message, outLen);
        }
        catch (Exception ex)
        {
            // No exception or partial C# crosses the ABI. Type-only diagnostics are deterministic.
            return Failure("decompiler_error:" + ex.GetType().Name, outLen);
        }
    }

    private static void SelectMethods(MetadataReader metadata, TypeDefinitionHandle handle, HashSet<int> methods, HashSet<TypeDefinitionHandle> types, int depth)
    {
        if (depth > 64 || !types.Add(handle))
            throw new NativeFailure("invalid_nested_scope");
        var type = metadata.GetTypeDefinition(handle);
        foreach (var method in type.GetMethods())
            methods.Add(MetadataTokens.GetToken(method));
        foreach (var child in type.GetNestedTypes())
            SelectMethods(metadata, child, methods, types, depth + 1);
    }

    private static byte[] Payload(string? reason, string? code, IEnumerable<(int MethodToken, int Offset, int Line)> mapping)
    {
        using var stream = new BoundedPayloadStream();
        using (var json = new Utf8JsonWriter(stream))
        {
            json.WriteStartObject();
            json.WriteBoolean("ok", reason == null);
            json.WriteString("reason", reason);
            json.WriteString("code", code);
            json.WriteStartArray("mapping");
            foreach (var (methodToken, offset, line) in mapping)
            {
                json.WriteStartObject();
                if (methodToken != 0)
                    json.WriteNumber("method_token", methodToken);
                json.WriteNumber("il_offset", offset);
                json.WriteNumber("line", line);
                json.WriteEndObject();
            }
            json.WriteEndArray();
            json.WriteEndObject();
        }
        return stream.ToArray();
    }

    private static byte* Failure(string reason, int* outLen)
    {
        try { return CopyOut(Payload(reason, null, Array.Empty<(int, int, int)>()), outLen); }
        catch (Exception) { *outLen = 0; return null; }
    }

    private static byte* CopyOut(byte[] bytes, int* outLen)
    {
        if (bytes.Length > MaxOutputBytes)
            return Failure("output_limit", outLen);
        byte* buffer = (byte*)NativeMemory.Alloc((nuint)bytes.Length);
        bytes.AsSpan().CopyTo(new Span<byte>(buffer, bytes.Length));
        *outLen = bytes.Length;
        return buffer;
    }
}

internal sealed class NativeFailure(string reason) : Exception(reason);

// Stop code/JSON growth before unbounded output allocations, not just before copying to C.
internal sealed class BoundedCodeWriter : TextWriter
{
    private readonly StringBuilder text = new();
    public override Encoding Encoding => Encoding.UTF8;
    public override void Write(char value)
    {
        Check(1);
        text.Append(value);
    }
    public override void Write(string? value)
    {
        if (value == null) return;
        Check(value.Length);
        text.Append(value);
    }
    public override void Write(char[] buffer, int index, int count)
    {
        Check(count);
        text.Append(buffer, index, count);
    }
    private void Check(int count)
    {
        if (count > Exports.MaxOutputBytes - text.Length)
            throw new NativeFailure("output_limit");
    }
    public override string ToString() => text.ToString();
}

internal sealed class BoundedPayloadStream : MemoryStream
{
    public override void Write(byte[] buffer, int offset, int count)
    {
        Check(count);
        base.Write(buffer, offset, count);
    }
    public override void Write(ReadOnlySpan<byte> buffer)
    {
        Check(buffer.Length);
        base.Write(buffer);
    }
    private void Check(int count)
    {
        if (count > Exports.MaxOutputBytes - Length)
            throw new NativeFailure("output_limit");
    }
}

internal sealed class RejectInvalidIL : IILTransform
{
    public void Run(ILFunction function, ILTransformContext context) => Check(function);

    internal static void Check(ILFunction function)
    {
        foreach (var instruction in function.Descendants)
        {
            if (instruction is ILFunction nested && nested.Warnings.Count != 0)
                throw new NativeFailure("il_warning");
            if (instruction is InvalidExpression || instruction is InvalidBranch)
                throw new NativeFailure("invalid_il");
        }
        if (function.Warnings.Count != 0)
            throw new NativeFailure("il_warning");
    }
}

// Only installed or explicitly configured core-framework metadata supplements the selected module.
// No sibling-module search, runtime assembly loading, GAC fallback or module resolution.
internal sealed class FrameworkResolver : IAssemblyResolver, IDisposable
{
    private readonly PEFile selectedModule;
    private readonly UniversalAssemblyResolver resolver;
    private readonly string[] directories;
    private readonly Dictionary<string, PEFile> files = new(StringComparer.Ordinal);

    internal FrameworkResolver(PEFile selectedModule)
    {
        this.selectedModule = selectedModule;
        // Distributed reference assemblies are metadata only. Never probe the
        // analyzed input's directory or load its code to resolve framework types.
        string installed = Environment.ProcessPath is string executable
            ? Path.Combine(Path.GetDirectoryName(executable)!, "dotnet-refs") : "";
        directories = (Environment.GetEnvironmentVariable("RXD_REFDIR") ?? Exports.InstalledReferenceDirectory ?? installed)
            .Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries)
            .Select(Path.GetFullPath).ToArray();
        resolver = new UniversalAssemblyResolver(null, false, null);
        foreach (string? directory in resolver.GetSearchDirectories().ToArray())
            resolver.RemoveSearchDirectory(directory);
        foreach (string directory in directories)
            resolver.AddSearchDirectory(directory);
    }

    public IDisposable? BeginSnapshot() => resolver.BeginSnapshot();

    public MetadataFile? Resolve(IAssemblyReference reference)
    {
        if (reference.Name == selectedModule.Name)
            return selectedModule;
        string key = Convert.ToHexString(reference.PublicKeyToken ?? Array.Empty<byte>());
        bool frameworkName = reference.Name.StartsWith("System.", StringComparison.Ordinal)
            || reference.Name.StartsWith("Microsoft.Win32.", StringComparison.Ordinal)
            || reference.Name is "System" or "mscorlib" or "netstandard" or "Microsoft.CSharp"
                or "Microsoft.VisualBasic" or "Microsoft.VisualBasic.Core";
        // ILSpy's implicit framework references have no public-key token.
        if (!frameworkName || key is not ("" or "B03F5F7F11D50A3A" or "B77A5C561934E089" or "7CEC85D7BEA7798E" or "CC7B13FFCD2DDD51"))
            throw new NativeFailure("cross_assembly_reference:" + reference.Name);
        string? path = resolver.FindAssemblyFile(reference);
        // Optional framework type-forward probes may miss (e.g. legacy permissions).
        // Let ILSpy retain unresolved-type diagnostics instead of rejecting the method.
        if (path == null)
            return null;
        if (!directories.Contains(Path.GetDirectoryName(Path.GetFullPath(path)), StringComparer.Ordinal))
            throw new NativeFailure("unresolved_framework_reference:" + reference.Name);
        if (!files.TryGetValue(path, out var file))
        {
            file = new PEFile(path);
            files.Add(path, file);
        }
        return file;
    }

    public MetadataFile ResolveModule(MetadataFile mainModule, string moduleName)
        => throw new NativeFailure("cross_module_reference");
    public Task<MetadataFile?> ResolveAsync(IAssemblyReference reference)
        => Task.FromResult<MetadataFile?>(Resolve(reference));
    public Task<MetadataFile?> ResolveModuleAsync(MetadataFile mainModule, string moduleName)
        => Task.FromResult<MetadataFile?>(ResolveModule(mainModule, moduleName));
    public void Dispose()
    {
        foreach (var file in files.Values)
            file.Dispose();
    }
}
