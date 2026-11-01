// Optional development comparator; never load/execute the analyzed assembly.
// Dependency resolution is disabled to match rxd's single-module query scope.
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
using System.Security.Cryptography;
using System.Text.Json;
using ICSharpCode.Decompiler;
using ICSharpCode.Decompiler.CSharp;
using ICSharpCode.Decompiler.Metadata;

if (args.Length != 2)
    throw new ArgumentException("usage: ilspy-method FILE METHOD_TOKEN_HEX");
var bytes = File.ReadAllBytes(args[0]);
var token = Convert.ToInt32(args[1], 16);
var handle = MetadataTokens.EntityHandle(token);
if (handle.Kind != HandleKind.MethodDefinition)
    throw new ArgumentException("expected MethodDef token");
using var pe = new PEFile(args[0], new MemoryStream(bytes), PEStreamOptions.PrefetchEntireImage);
var definition = pe.Metadata.GetMethodDefinition((MethodDefinitionHandle)handle);
var body = pe.Reader.GetMethodBody(definition.RelativeVirtualAddress);
var settings = new DecompilerSettings {
    ThrowOnAssemblyResolveErrors = false,
    UseRefLocalsForAccurateOrderOfEvaluation = true,
};
var decompiler = new CSharpDecompiler(pe, new NoDependencies(), settings);
var code = decompiler.DecompileAsString(handle);
Console.WriteLine(JsonSerializer.Serialize(new {
    source_id = Convert.ToHexString(SHA256.HashData(bytes)).ToLowerInvariant(),
    method_token = token,
    il_hex = Convert.ToHexString(body.GetILBytes()!).ToLowerInvariant(),
    handlers = body.ExceptionRegions.Select(h => new {
        kind = h.Kind.ToString().ToLowerInvariant(),
        try_offset = h.TryOffset, try_length = h.TryLength,
        handler_offset = h.HandlerOffset, handler_length = h.HandlerLength,
        class_token = h.Kind == ExceptionRegionKind.Catch ? (int?)MetadataTokens.GetToken(h.CatchType) : null,
        filter_offset = h.Kind == ExceptionRegionKind.Filter ? (int?)h.FilterOffset : null,
    }),
    code,
}));

sealed class NoDependencies : IAssemblyResolver {
    public IDisposable? BeginSnapshot() => null;
    public MetadataFile? Resolve(IAssemblyReference reference) => null;
    public MetadataFile? ResolveModule(MetadataFile mainModule, string moduleName) => null;
    public Task<MetadataFile?> ResolveAsync(IAssemblyReference reference) => Task.FromResult<MetadataFile?>(null);
    public Task<MetadataFile?> ResolveModuleAsync(MetadataFile mainModule, string moduleName) => Task.FromResult<MetadataFile?>(null);
}
