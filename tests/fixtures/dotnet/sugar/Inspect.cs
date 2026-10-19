// Independent expectation generator: PE metadata reads only, never loads target code.
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
using System.Security.Cryptography;
using System.Text.Json;

byte[] image = File.ReadAllBytes(args[0]);
using var pe = new PEReader(new MemoryStream(image));
MetadataReader metadata = pe.GetMetadataReader();

string TypeName(TypeDefinitionHandle handle)
{
    TypeDefinition type = metadata.GetTypeDefinition(handle);
    string name = metadata.GetString(type.Name);
    TypeDefinitionHandle parent = type.GetDeclaringType();
    if (!parent.IsNil)
        return TypeName(parent) + "+" + name;
    string ns = metadata.GetString(type.Namespace);
    return ns.Length == 0 ? name : ns + "." + name;
}

var methods = new SortedDictionary<string, object>();
foreach (MethodDefinitionHandle handle in metadata.MethodDefinitions)
{
    MethodDefinition method = metadata.GetMethodDefinition(handle);
    if (method.RelativeVirtualAddress == 0)
        throw new InvalidDataException("Fixture methods must have original IL bodies");
    MethodBodyBlock body = pe.GetMethodBody(method.RelativeVirtualAddress);
    var handlers = body.ExceptionRegions.Select(region => new
    {
        kind = region.Kind.ToString().ToLowerInvariant(),
        try_offset = region.TryOffset,
        try_length = region.TryLength,
        handler_offset = region.HandlerOffset,
        handler_length = region.HandlerLength,
        class_token = region.Kind == ExceptionRegionKind.Catch
            ? (int?)MetadataTokens.GetToken(region.CatchType) : null,
        filter_offset = region.Kind == ExceptionRegionKind.Filter ? (int?)region.FilterOffset : null,
    }).ToArray();
    byte[] il = body.GetILBytes()!;
    methods.Add($"0x{MetadataTokens.GetToken(handle):x8}", new
    {
        name = metadata.GetString(method.Name),
        declaring_type = TypeName(method.GetDeclaringType()),
        rva = method.RelativeVirtualAddress,
        il_hex = Convert.ToHexString(il).ToLowerInvariant(),
        code_size = il.Length,
        max_stack = body.MaxStack,
        init_locals = body.LocalVariablesInitialized,
        local_signature_token = body.LocalSignature.IsNil ? 0 : MetadataTokens.GetToken(body.LocalSignature),
        handlers,
    });
}
Console.WriteLine(JsonSerializer.Serialize(new
{
    sha256 = Convert.ToHexString(SHA256.HashData(image)).ToLowerInvariant(),
    bytes = image.Length,
    mvid = metadata.GetGuid(metadata.GetModuleDefinition().Mvid),
    pe_timestamp = $"0x{pe.PEHeaders.CoffHeader.TimeDateStamp:x8}",
    methods,
}, new JsonSerializerOptions { WriteIndented = true }));
