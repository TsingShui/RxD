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
    if (method.RelativeVirtualAddress == 0) {
        methods.Add($"0x{MetadataTokens.GetToken(handle):x8}", new {
            name = metadata.GetString(method.Name),
            declaring_type = TypeName(method.GetDeclaringType()),
            rva = 0, il_hex = (string?)null, code_size = 0, handlers = Array.Empty<object>()
        });
        continue;
    }
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
var types = new SortedDictionary<string, object>();
var properties = new SortedDictionary<string, object>();
var events = new SortedDictionary<string, object>();
foreach (TypeDefinitionHandle handle in metadata.TypeDefinitions) {
    TypeDefinition type = metadata.GetTypeDefinition(handle);
    types.Add($"0x{MetadataTokens.GetToken(handle):x8}", new {
        name = TypeName(handle),
        parent_token = type.GetDeclaringType().IsNil ? (int?)null : MetadataTokens.GetToken(type.GetDeclaringType()),
        extends_token = type.BaseType.IsNil ? (int?)null : MetadataTokens.GetToken(type.BaseType),
        flags = (int)type.Attributes,
        interfaces = type.GetInterfaceImplementations().Select(h => MetadataTokens.GetToken(metadata.GetInterfaceImplementation(h).Interface)).ToArray(),
        generic_parameters = type.GetGenericParameters().Select(h => new { name = metadata.GetString(metadata.GetGenericParameter(h).Name), number = metadata.GetGenericParameter(h).Index }).ToArray()
    });
    foreach (PropertyDefinitionHandle propertyHandle in type.GetProperties()) {
        var property = metadata.GetPropertyDefinition(propertyHandle);
        var accessors = property.GetAccessors();
        var methodsForProperty = new List<object>();
        if (!accessors.Getter.IsNil) methodsForProperty.Add(new { method_token = MetadataTokens.GetToken(accessors.Getter), semantics = 2 });
        if (!accessors.Setter.IsNil) methodsForProperty.Add(new { method_token = MetadataTokens.GetToken(accessors.Setter), semantics = 1 });
        properties.Add($"0x{MetadataTokens.GetToken(propertyHandle):x8}", new {
            name = metadata.GetString(property.Name), owner_token = MetadataTokens.GetToken(handle),
            signature_hex = Convert.ToHexString(metadata.GetBlobBytes(property.Signature)).ToLowerInvariant(),
            accessors = methodsForProperty
        });
    }
    foreach (EventDefinitionHandle eventHandle in type.GetEvents()) {
        var definition = metadata.GetEventDefinition(eventHandle); var accessors = definition.GetAccessors();
        var methodsForEvent = new List<object>();
        if (!accessors.Adder.IsNil) methodsForEvent.Add(new { method_token = MetadataTokens.GetToken(accessors.Adder), semantics = 8 });
        if (!accessors.Remover.IsNil) methodsForEvent.Add(new { method_token = MetadataTokens.GetToken(accessors.Remover), semantics = 16 });
        if (!accessors.Raiser.IsNil) methodsForEvent.Add(new { method_token = MetadataTokens.GetToken(accessors.Raiser), semantics = 32 });
        events.Add($"0x{MetadataTokens.GetToken(eventHandle):x8}", new {
            name = metadata.GetString(definition.Name), owner_token = MetadataTokens.GetToken(handle),
            event_type_token = MetadataTokens.GetToken(definition.Type), accessors = methodsForEvent
        });
    }
}
int FileOffset(int rva) { var section = pe.PEHeaders.SectionHeaders.Single(section => rva >= section.VirtualAddress && rva < section.VirtualAddress + section.SizeOfRawData); return section.PointerToRawData + rva - section.VirtualAddress; }
int metadataOffset = FileOffset(pe.PEHeaders.CorHeader!.MetadataDirectory.RelativeVirtualAddress);
var table_file_offsets = new { nested_class = metadataOffset + metadata.GetTableMetadataOffset(TableIndex.NestedClass), method_def = metadataOffset + metadata.GetTableMetadataOffset(TableIndex.MethodDef), method_semantics = metadataOffset + metadata.GetTableMetadataOffset(TableIndex.MethodSemantics) };
Console.WriteLine(JsonSerializer.Serialize(new
{
    sha256 = Convert.ToHexString(SHA256.HashData(image)).ToLowerInvariant(),
    bytes = image.Length,
    mvid = metadata.GetGuid(metadata.GetModuleDefinition().Mvid),
    pe_timestamp = $"0x{pe.PEHeaders.CoffHeader.TimeDateStamp:x8}",
    methods, types, properties, events, table_file_offsets,
}, new JsonSerializerOptions { WriteIndented = true }));
