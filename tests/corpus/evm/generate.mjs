import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import solc from 'solc';

const root = path.dirname(new URL(import.meta.url).pathname);
const sha256 = b => crypto.createHash('sha256').update(b).digest('hex');
const json = (p, obj) => fs.writeFileSync(path.join(root, p), JSON.stringify(obj, null, 2) + '\n');
const versions = ['solc', '@openzeppelin/contracts'].map(name => {
    const p = JSON.parse(fs.readFileSync(path.join(root, 'node_modules', name, 'package.json')));
    const lock = JSON.parse(fs.readFileSync(path.join(root, 'package-lock.json'))).packages['node_modules/' + name];
    return {name, version:p.version, license:p.license, resolved:lock.resolved, integrity:lock.integrity};
});
const settings = {
    optimizer: { enabled: true, runs: 200 },
    viaIR: true,
    evmVersion: 'cancun',
    metadata: { appendCBOR: true, bytecodeHash: 'ipfs', useLiteralContent: true },
    debug: { revertStrings: 'default', debugInfo: ['location', 'snippet'] },
    outputSelection: { '*': {'*': ['abi','metadata','ir','irOptimized','storageLayout','evm.assembly','evm.legacyAssembly','evm.methodIdentifiers','evm.deployedBytecode'], '': ['ast']} }
};
if (!solc.version().startsWith('0.8.26+commit.8a97fa7a')) throw Error('Unexpected compiler: ' + solc.version());
const names = new Map([
    [0x00,'STOP'],[0x01,'ADD'],[0x02,'MUL'],[0x03,'SUB'],[0x04,'DIV'],[0x05,'SDIV'],[0x06,'MOD'],[0x07,'SMOD'],[0x08,'ADDMOD'],[0x09,'MULMOD'],[0x0a,'EXP'],[0x0b,'SIGNEXTEND'],
    [0x10,'LT'],[0x11,'GT'],[0x12,'SLT'],[0x13,'SGT'],[0x14,'EQ'],[0x15,'ISZERO'],[0x16,'AND'],[0x17,'OR'],[0x18,'XOR'],[0x19,'NOT'],[0x1a,'BYTE'],[0x1b,'SHL'],[0x1c,'SHR'],[0x1d,'SAR'],[0x20,'KECCAK256'],
    [0x30,'ADDRESS'],[0x31,'BALANCE'],[0x32,'ORIGIN'],[0x33,'CALLER'],[0x34,'CALLVALUE'],[0x35,'CALLDATALOAD'],[0x36,'CALLDATASIZE'],[0x37,'CALLDATACOPY'],[0x38,'CODESIZE'],[0x39,'CODECOPY'],[0x3a,'GASPRICE'],[0x3b,'EXTCODESIZE'],[0x3c,'EXTCODECOPY'],[0x3d,'RETURNDATASIZE'],[0x3e,'RETURNDATACOPY'],[0x3f,'EXTCODEHASH'],
    [0x40,'BLOCKHASH'],[0x41,'COINBASE'],[0x42,'TIMESTAMP'],[0x43,'NUMBER'],[0x44,'PREVRANDAO'],[0x45,'GASLIMIT'],[0x46,'CHAINID'],[0x47,'SELFBALANCE'],[0x48,'BASEFEE'],[0x49,'BLOBHASH'],[0x4a,'BLOBBASEFEE'],
    [0x50,'POP'],[0x51,'MLOAD'],[0x52,'MSTORE'],[0x53,'MSTORE8'],[0x54,'SLOAD'],[0x55,'SSTORE'],[0x56,'JUMP'],[0x57,'JUMPI'],[0x58,'PC'],[0x59,'MSIZE'],[0x5a,'GAS'],[0x5b,'JUMPDEST'],[0x5c,'TLOAD'],[0x5d,'TSTORE'],[0x5e,'MCOPY'],[0x5f,'PUSH0'],
    [0xf0,'CREATE'],[0xf1,'CALL'],[0xf2,'CALLCODE'],[0xf3,'RETURN'],[0xf4,'DELEGATECALL'],[0xf5,'CREATE2'],[0xfa,'STATICCALL'],[0xfd,'REVERT'],[0xfe,'INVALID'],[0xff,'SELFDESTRUCT']
]);
for(let i=1;i<=32;i++) names.set(0x5f+i,'PUSH'+i);
for(let i=1;i<=16;i++){names.set(0x7f+i,'DUP'+i);names.set(0x8f+i,'SWAP'+i);}
for(let i=0;i<=4;i++)names.set(0xa0+i,'LOG'+i);
function decode(bytes) {
    const out=[];
    for(let offset=0;offset<bytes.length;) {
        const opcode=bytes[offset];const size=opcode>=0x60&&opcode<=0x7f?opcode-0x5f:0;
        const next=Math.min(bytes.length,offset+1+size);
        const raw=bytes.subarray(offset,next);
        out.push({offset,end:next,opcode,nm:names.get(opcode)??'UNDEFINED',raw:raw.toString('hex'),...(size?{immediate:raw.subarray(1).toString('hex'),truncated:raw.length!==size+1}:{})});
        offset=next;
    }
    return out;
}
function sourceMap(entries) {
    let prev=[-1,-1,-1,'',0];
    return entries.split(';').map(e=>{const parts=e.split(':');prev=prev.map((v,i)=>parts[i]===undefined||parts[i]===''?v:i===3?parts[i]:Number(parts[i]));return {start:prev[0],length:prev[1],source_id:prev[2],jump:prev[3],modifier_depth:prev[4]};});
}
function walk(node,cb) {
    if(!node||typeof node!=='object')return;
    if(node.nodeType)cb(node);
    for(const v of Object.values(node)) if(Array.isArray(v))v.forEach(n=>walk(n,cb));else if(v&&typeof v==='object')walk(v,cb);
}
// Restricted independent CFG oracle: only adjacent PUSH destinations are
// resolved. Indirect jumps stay unknown; this does not recover subroutine
// returns or assert arbitrary runtime reachability/semantic equivalence.
function staticRegions(instructions,dispatch,maps) {
    const code=instructions.slice(0,maps.length);
    const starts=new Set([0]);
    const stops=new Set(['STOP','RETURN','REVERT','INVALID','SELFDESTRUCT','JUMP','JUMPI']);
    for(let i=0;i<code.length;i++){
        if(code[i].nm==='JUMPDEST')starts.add(code[i].offset);
        if(stops.has(code[i].nm)&&i+1<code.length)starts.add(code[i+1].offset);
    }
    const offsets=[...starts].sort((a,b)=>a-b),blocks=new Map();
    for(let i=0;i<offsets.length;i++){
        const start=offsets[i],end=offsets[i+1]??code.at(-1).end;
        const ins=code.filter(x=>x.offset>=start&&x.offset<end),last=ins.at(-1),prior=ins.at(-2),successors=[];
        let unknown_jump=false;
        if(['JUMP','JUMPI'].includes(last.nm)){
            if(prior?.immediate){const target=Number.parseInt(prior.immediate,16);if(code.find(x=>x.offset===target)?.nm==='JUMPDEST')successors.push(target);else unknown_jump=true;}else unknown_jump=true;
            if(last.nm==='JUMPI')successors.push(end);
        }else if(!stops.has(last.nm)&&end<=code.at(-1).offset)successors.push(end);
        blocks.set(start,{start,end,successors,unknown_jump});
    }
    return dispatch.flatMap(d=>d.occurrences.filter(o=>o.entry_offset!==undefined).map(o=>{
        const work=[o.entry_offset],seen=new Set(),ranges=[];
        while(work.length){const offset=work.pop();if(seen.has(offset))continue;seen.add(offset);const block=blocks.get(offset);if(!block)continue;ranges.push(block);work.push(...block.successors);}
        ranges.sort((a,b)=>a.start-b.start);
        return {signature:d.signature,selector:d.selector,entry_offset:o.entry_offset,blocks:ranges,has_unresolved_jump:ranges.some(b=>b.unknown_jump)};
    }));
}
const manifest={generator:'generate.mjs (independent solc output, source AST/sourceMap and byte decoder; no rxd)',node:process.version,compiler:solc.version(),compiler_component_sha256:{soljson_js:sha256(fs.readFileSync(path.join(root,'node_modules/solc/soljson.js')))},compiler_licenses:{solc_js_wrapper:'MIT',solidity_compiler:'GPL-3.0',compiler_license_url:'https://github.com/ethereum/solidity/blob/v0.8.26/LICENSE.txt'},packages:versions,settings,samples:[]};
for(const [file,contract,slug,viaIR] of [['NavigationCorpus.sol','NavigationCorpus','authored-navigation',true],['NavigationCorpus.sol','NavigationCorpus','authored-navigation-legacy',false],['PinnedToken.sol','PinnedToken','openzeppelin-erc20',true]]) {
    const sources={};
    function collect(key) {
        if(sources[key])return;
        const p=key.startsWith('@')?path.join(root,'node_modules',key):path.join(root,key);
        const content=fs.readFileSync(p,'utf8');sources[key]={content};
        for(const m of content.matchAll(/import\s+(?:[^;]*?\s+from\s+)?["']([^"']+)["'];/g)) {
            const imp=m[1];collect(imp.startsWith('.')?path.posix.normalize(path.posix.join(path.posix.dirname(key),imp)):imp);
        }
    }
    collect(file);
    const sampleSettings={...settings,viaIR};
    const input={language:'Solidity',sources,settings:sampleSettings};
    const output=JSON.parse(solc.compile(JSON.stringify(input)));
    const errors=(output.errors??[]).filter(e=>e.severity==='error');if(errors.length)throw Error(errors.map(e=>e.formattedMessage).join('\n'));
    const c=output.contracts[file][contract];
    const bytecode=c.evm.deployedBytecode;
    const runtime=Buffer.from(bytecode.object,'hex');
    fs.writeFileSync(path.join(root,slug+'.runtime.hex'),bytecode.object+'\n');
    fs.writeFileSync(path.join(root,slug+'.runtime.bin'),runtime);
    json(slug+'.input.json',input);
    json(slug+'.compiler-output.json',output);
    fs.writeFileSync(path.join(root,slug+'.assembly.txt'),c.evm.assembly);
    fs.writeFileSync(path.join(root,slug+'.ir.yul'),c.irOptimized);
    const instructions=decode(runtime),maps=sourceMap(bytecode.sourceMap);
    instructions.forEach((ins,i)=>{if(i<maps.length)ins.source=maps[i];});
    const funcs=[];
    for(const [sourceName,src] of Object.entries(output.sources)) {
        walk(src.ast,node=>{
            if(node.nodeType!=='FunctionDefinition'||!node.implemented)return;
            const [start,length,source_id]=node.src.split(':').map(Number);
            const body=node.body.src.split(':').map(Number);
            const offsets=instructions.filter(ins=>ins.source?.source_id===source_id&&ins.source.start>=body[0]&&ins.source.start+ins.source.length<=body[0]+body[1]).map(i=>i.offset);
            const control=[];
            walk(node.body,n=>{if(['IfStatement','ForStatement','WhileStatement','DoWhileStatement','Return','InlineAssembly','Assignment'].includes(n.nodeType)){const [s,l]=n.src.split(':').map(Number);control.push({kind:n.nodeType,start:s,length:l,snippet:Buffer.from(sources[sourceName].content).subarray(s,s+l).toString('utf8')});}});
            funcs.push({source:sourceName,source_id,ast_id:node.id,name:node.name,kind:node.kind,visibility:node.visibility,mutability:node.stateMutability,start,length,body_start:body[0],body_length:body[1],body_instruction_offsets:offsets,control});
        });
    }
    const dispatch=[];
    for(const [signature,selector]of Object.entries(c.evm.methodIdentifiers)) {
        const occurrences=[];
        for(let i=0;i<instructions.length;i++) {
            const ins=instructions[i];if(ins.nm!=='PUSH4'||ins.immediate!==selector)continue;
            const seq=instructions.slice(i,i+4);
            occurrences.push({push_offset:ins.offset,pattern:seq.map(x=>x.nm),...(seq[1]?.nm==='EQ'&&seq[2]?.immediate&&seq[3]?.nm==='JUMPI'?{entry_offset:Number.parseInt(seq[2].immediate,16),jumpi_offset:seq[3].offset}:{})});
        }
        dispatch.push({signature,selector,occurrences});
    }
    const metadata_length=runtime.readUInt16BE(runtime.length-2);
    const data=[];
    for(const [label,hex]of Object.entries(c.evm.legacyAssembly?.['.data']?.['0']?.['.data']??{})) {
        if(typeof hex!=='string')continue;
        const b=Buffer.from(hex,'hex'),ranges=[];
        for(let offset=runtime.indexOf(b);offset>=0;offset=runtime.indexOf(b,offset+1))ranges.push({start:offset,end:offset+b.length});
        data.push({compiler_label:label,hex,utf8:b.toString('utf8'),ranges});
    }
    const oracle={sample:slug,compiler:solc.version(),method_identifiers:c.evm.methodIdentifiers,dispatch,storage_layout:c.storageLayout,function_debug_data:bytecode.functionDebugData,source_map_instruction_count:maps.length,restricted_static_regions:staticRegions(instructions,dispatch,maps),compiler_data:data,metadata:{start:runtime.length-metadata_length-2,end:runtime.length,cbor_length:metadata_length,hex:runtime.subarray(runtime.length-metadata_length-2).toString('hex')},functions:funcs,instructions};
    json(slug+'.oracle.json',oracle);
    const hashes=Object.fromEntries(Object.entries(sources).map(([name,{content}])=>[name,sha256(content)]));
    manifest.samples.push({slug,root_source:file,contract,settings:sampleSettings,runtime_bytes:runtime.length,runtime_sha256:sha256(runtime),hex_carrier_sha256:sha256(bytecode.object+'\n'),source_sha256:hashes,source_count:Object.keys(sources).length,selectors:c.evm.methodIdentifiers,warnings:output.errors??[]});
    console.log(slug,runtime.length,'bytes',JSON.stringify(dispatch));
}
manifest.generator_sha256=sha256(fs.readFileSync(path.join(root,'generate.mjs')));
manifest.package_lock_sha256=sha256(fs.readFileSync(path.join(root,'package-lock.json')));
json('manifest.json',manifest);
