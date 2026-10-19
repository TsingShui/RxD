#!/usr/bin/env python3
"""Original ECMA-335 bytes and independent explicit-operand expectations; never execute."""
from pathlib import Path
import argparse, hashlib, json, struct
parser=argparse.ArgumentParser()
parser.add_argument("--expanded-signature",action="store_true")
parser.add_argument("--output",type=Path)
options=parser.parse_args()
ROOT=options.output or Path(__file__).resolve().parent
ROOT.mkdir(parents=True,exist_ok=True)
u16=lambda n: struct.pack('<H',n)
u32=lambda n: struct.pack('<I',n)
align=lambda b,n: b+bytes(-len(b)%n)
strings=bytearray(b'\0'); blobs=bytearray(b'\0')
def name(s):
    offset=len(strings); strings.extend(s.encode()+b'\0'); return offset
def compressed(n):
    return bytes([n]) if n<128 else bytes([0x80|(n>>8),n&255]) if n<16384 else bytes([0xc0|(n>>24),(n>>16)&255,(n>>8)&255,n&255])
def blob(b):
    offset=len(blobs); blobs.extend(compressed(len(b))+b); return offset
module=name('References.dll'); assembly=name('ReferenceFixture'); ns=name('Proof'); target=name('Target'); generic=name('Holder`1'); inner=name('Inner'); object_=name('Object'); system=name('System')
# TypeRef: local target, external target from two distinct scopes, local Holder,
# local nested Inner, and the Target type in a same-named external module.
type_refs=[(4,target,ns),(6,target,ns),(10,target,ns),(4,generic,ns),(7,inner,0),(5,target,ns),(0,target,ns)]
int_sig=b'\0\x01\x08\x08'; string_sig=b'\0\x01\x0e\x0e'; ctor_sig=b'\x20\0\x01'; generic_sig=b'\x10\x01\x01\x1e\0\x1e\0'
echo_sig=(b'\x20'+compressed(100)+b'\x13\0'+b'\x13\0'*100) if options.expanded_signature else b'\x20\x01\x13\0\x13\0'; void_sig=b'\0\0\x01'
# Parent-coded MemberRefs are independently chosen table/RID operands.
member_refs=[(9,'Pick',int_sig),(9,'Pick',string_sig),(16,'.ctor',ctor_sig),(17,'Pick',int_sig),(25,'Pick',int_sig),(41,'Touch',void_sig),(12,'Echo',echo_sig if options.expanded_signature else b'\x20\x01\x08\x08'),(20,'Echo',b'\x20\x01\x0e\x0e'),(16,'Dup',int_sig),(10,'Pick',int_sig),(16,'Generic',generic_sig),(41,'AliasType',b'\0\x01\x12\x08\x12\x08'),(41,'AliasType',b'\0\x01\x12\x05\x12\x05'),(41,'AliasType',b'\0\x01\x12\x09\x12\x09'),(41,'TypeSpecEcho',b'\0\x01\x12\x06\x12\x06'),(41,'TypeSpecEcho',b'\0\x01\x12\x0a\x12\x0a'),(57,'Pick',int_sig)]
# Field aliases preserve complete signatures, constructed owners and scopes.
member_refs.extend([(9,'Value',b'\x06\x08'),(9,'Value',b'\x06\x0e'),(12,'Item',b'\x06\x08'),(20,'Item',b'\x06\x0e'),(17,'Value',b'\x06\x08'),(41,'Value',b'\x06\x08'),(9,'DupField',b'\x06\x08'),(25,'Value',b'\x06\x08'),(12,'Item',b'\x06\x0e'),(57,'Value',b'\x06\x08'),(17,'Virtual',ctor_sig)])
code=bytearray(); sites=[]
def site(op, mnemonic, token):
    offset=len(code); code.extend(op+u32(token)); sites.append(dict(il_offset=offset,opcode=mnemonic,target_token=token))
for op,mnemonic in [(b'\x28','call'),(b'\xfe\x06','ldftn'),(b'\xd0','ldtoken'),(b'\x27','jmp')]: site(op,mnemonic,0x0a000001)
for token in [0x0a000002,0x0a000004,0x0a000005,0x0a000006,0x0a000007,0x0a000008,0x0a000009,0x0a00000a]: site(b'\x28','call',token)
site(b'\x73','newobj',0x0a000003)
for token in [0x2b000001,0x2b000002,0x2b000003]: site(b'\x28','call',token)
for token in [0x0a00000c,0x0a00000d,0x0a00000e,0x0a00000f,0x0a000010,0x0a000011]: site(b'\x28','call',token)
code.extend(b'\x2a')
methods=[('Pick',int_sig,b'\x02\x2a',0x16),('Pick',string_sig,b'\x02\x2a',0x16),('.ctor',ctor_sig,b'\x2a',6),('Generic',generic_sig,b'\x02\x2a',0x16),('Dup',int_sig,b'\x02\x2a',0x16),('Dup',int_sig,b'\x02\x2a',0x16),('Caller',void_sig,bytes(code),0x16),('Echo',echo_sig,b'\x03\x2a',6),('Touch',void_sig,b'\x2a',0x16),('AliasType',b'\0\x01\x12\x08\x12\x08',b'\x02\x2a',0x16),('TypeSpecEcho',b'\0\x01\x12\x06\x12\x06',b'\x02\x2a',0x16)]
field_type_code=bytearray(); field_type_sites=[]
def field_type_site(op, mnemonic, token):
    offset=len(field_type_code); field_type_code.extend(op+u32(token)); field_type_sites.append(dict(method_token=0x0600000c,il_offset=offset,opcode=mnemonic,target_token=token))
for op,mnemonic in [(b'\x7b','ldfld'),(b'\x7c','ldflda'),(b'\x7d','stfld'),(b'\xd0','ldtoken')]: field_type_site(op,mnemonic,0x0a000012)
for token in range(0x0a000013,0x0a00001c): field_type_site(b'\x7b','ldfld',token)
for op,mnemonic in [(b'\x7e','ldsfld'),(b'\x7f','ldsflda'),(b'\x80','stsfld')]: field_type_site(op,mnemonic,0x04000001)
for token in [0x02000002,0x01000001,0x01000002,0x01000003,0x01000005,0x01000006,0x01000007,0x1b000001,0x1b000002,0x1b000003,0x1b000004,0x1b000005,0x1b000006]: field_type_site(b'\xd0','ldtoken',token)
field_type_site(b'\x74','castclass',0x01000001)
field_type_site(b'\xfe\x16','constrained.',0x1b000001)
field_type_site(b'\x6f','callvirt',0x0a00001c)
field_type_code.extend(b'\x2a')
methods.append(('FieldTypeCaller',void_sig,bytes(field_type_code),0x16))
text=bytearray(bytes(72)); method_rows=[]; param_rows=[]; expected_methods=[]
for i,(n,sig,il,flags) in enumerate(methods,1):
    text[:]=align(bytes(text),4); rva=0x2000+len(text)
    body=bytes([(len(il)<<2)|2])+il if len(il)<64 else u16(0x3003)+u16(8)+u32(len(il))+u32(0)+il
    text.extend(body)
    method_rows.append(u32(rva)+u16(0)+u16(flags)+u16(name(n))+u16(blob(sig))+u16(1))
    expected_methods.append(dict(token=0x06000000|i,name=n,il_hex=il.hex(),rva=rva))
rows={
0:[u16(0)+u16(module)+u16(1)+u16(0)+u16(0)],
1:[u16(scope)+u16(n)+u16(namespace) for scope,n,namespace in type_refs],
2:[u32(0)+u16(name('<Module>'))+u16(0)+u16(0)+u16(1)+u16(1),u32(0x00100181)+u16(target)+u16(ns)+u16(0)+u16(1)+u16(1),u32(0x00100101)+u16(generic)+u16(ns)+u16(0)+u16(6)+u16(8),u32(0x00100102)+u16(inner)+u16(0)+u16(0)+u16(7)+u16(9)],
4:[u16(6)+u16(name(n))+u16(blob(sig)) for n,sig in [('Value',b'\x06\x08'),('Value',b'\x06\x0e'),('DupField',b'\x06\x08'),('DupField',b'\x06\x08'),('Unused',b'\x06\x08'),('Item',b'\x06\x13\0'),('Value',b'\x06\x08')]],
6:method_rows,
10:[u16(parent)+u16(name(n))+u16(blob(sig)) for parent,n,sig in member_refs],
26:[u16(module)],
27:[u16(blob(b'\x15\x12\x0c\x01'+(b'\x1b\0'+compressed(1000)+b'\x01'+b'\x08'*1000 if options.expanded_signature else b'\x08'))),u16(blob(b'\x15\x12\x0c\x01\x0e')),u16(blob(b'\x1d\x12\x08')),u16(blob(b'\x0f\x12\x08')),u16(blob(b'\x13\0')),u16(blob(b'\x15\x12\x11\x01\x08'))],
32:[u32(0x8004)+u16(1)+u16(0)+u16(0)+u16(0)+u32(0)+u16(0)+u16(assembly)+u16(0)],
35:[u16(version)+u16(0)+u16(0)+u16(0)+u32(0)+u16(0)+u16(assembly)+u16(0)+u16(0) for version in [2,3]],
41:[u16(4)+u16(2)],
42:[u16(0)+u16(0)+u16(3<<1)+u16(name('T')),u16(0)+u16(0)+u16((4<<1)|1)+u16(name('M'))],
43:[u16(4<<1)+u16(blob(b'\x0a\x01\x08')),u16(4<<1)+u16(blob(b'\x0a\x01\x0e')),u16((11<<1)|1)+u16(blob(b'\x0a\x01\x08'))],
}
valid=sum(1<<t for t in rows); tables=u32(0)+bytes([2,0,0,1])+struct.pack('<Q',valid)+bytes(8)
tables+=b''.join(u32(len(rows[t])) for t in sorted(rows))+b''.join(b''.join(rows[t]) for t in sorted(rows))
# 1,002 valid literals followed by a referenced final literal; boundary tests can
# corrupt the final entry, beyond dotscope's first-1,000-entry content pass.
us=b'\0'+b'\x03x\0\0'*1002
streams=[('#~',tables),('#Strings',bytes(strings)),('#Blob',bytes(blobs)),('#GUID',bytes.fromhex('112233445566778899aabbccddeeff00')),('#US',us)]
version=align(b'v4.0.30319\0',4); prefix=u32(0x424a5342)+u16(1)+u16(1)+u32(0)+u32(len(version))+version+u16(0)+u16(len(streams))
headers=bytearray(); payload=bytearray(); cursor=len(prefix)+sum(8+len(align(n.encode()+b'\0',4)) for n,_ in streams); stream_offsets={}
for n,b in streams:
    b=align(b,4); stream_offsets[n]=cursor+len(payload); headers.extend(u32(cursor+len(payload))+u32(len(b))+align(n.encode()+b'\0',4)); payload.extend(b)
metadata=prefix+headers+payload; text[:]=align(bytes(text),4); metadata_offset=512+len(text); metadata_rva=0x2000+len(text); text.extend(metadata)
text[:72]=u32(72)+u16(2)+u16(5)+u32(metadata_rva)+u32(len(metadata))+u32(1)+u32(0)+bytes(48)
raw_size=len(align(bytes(text),512)); virtual_size=len(text); dos=bytearray(128); dos[:2]=b'MZ'; dos[0x3c:0x40]=u32(128)
coff=u16(0x14c)+u16(1)+bytes(12)+u16(224)+u16(0x2102); opt=bytearray(224); opt[:2]=u16(0x10b); opt[4:8]=u32(raw_size); opt[20:24]=u32(0x2000); opt[28:32]=u32(0x400000); opt[32:40]=u32(0x2000)+u32(512); opt[40:44]=u16(4)+u16(0); opt[48:52]=u16(4)+u16(0); opt[56:64]=u32(((0x2000+virtual_size+0x1fff)//0x2000)*0x2000)+u32(512); opt[68:72]=u16(3)+u16(0x8540); opt[72:88]=u32(0x100000)+u32(0x1000)+u32(0x100000)+u32(0x1000); opt[92:96]=u32(16); opt[96+14*8:96+15*8]=u32(0x2000)+u32(72)
section=b'.text\0\0\0'+u32(virtual_size)+u32(0x2000)+u32(raw_size)+u32(512)+bytes(12)+u32(0x60000020)
image=align(bytes(dos)+b'PE\0\0'+coff+opt+section,512)+align(bytes(text),512)
(ROOT/'References.dll').write_bytes(image)
(ROOT/'expected.json').write_text(json.dumps(dict(sha256=hashlib.sha256(image).hexdigest(),bytes=len(image),methods=expected_methods,sites=sites,field_type_sites=field_type_sites,stream_offsets={n:metadata_offset+o for n,o in stream_offsets.items()}),indent=2)+'\n')
print(hashlib.sha256(image).hexdigest(),len(image))
