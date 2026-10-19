// Independent Babel oracle for original TS/JSX/TSX source. Never execute analyzed inputs.
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from '@babel/parser';
const root=dirname(fileURLToPath(import.meta.url));
const hash=b=>createHash('sha256').update(b).digest('hex');
const version=JSON.parse(readFileSync(join(root,'node_modules/@babel/parser/package.json'))).version;
if(version!=='7.29.9') throw new Error('Unpinned Babel parser');
const lock=JSON.parse(readFileSync(join(root,'package-lock.json')));
const inputs=[
 {name:'babel-parser-types',file:'babel-parser.d.ts',needle:'BABEL_PARSER_SYNTAX_ERROR',pkg:'@babel/parser',path:'typings/babel-parser.d.ts'},
 {name:'babel-types-declarations',file:'babel-types.d.ts',needle:'StringLiteral',pkg:'@babel/types',path:'lib/index.d.ts'},
 {name:'typed-login-source',file:'login.tsx',needle:'/api/login'},
 {name:'jsx-page-source',file:'page.jsx',needle:'A & B'},
];
const contextKinds={VariableDeclaration:'variable_declaration',VariableDeclarator:'variable_declarator',AssignmentExpression:'assignment_expression',UpdateExpression:'update_expression',ObjectProperty:'object_property',ClassProperty:'class_field',ClassPrivateProperty:'class_field',JSXElement:'jsx_element',JSXFragment:'jsx_fragment',JSXOpeningElement:'jsx_opening_element',JSXClosingElement:'jsx_closing_element',JSXAttribute:'jsx_attribute',JSXSpreadAttribute:'jsx_spread_attribute',JSXExpressionContainer:'jsx_expression',JSXText:'jsx_text',TSPropertySignature:'property_signature',TSTypeParameter:'type_parameter',TSCallSignatureDeclaration:'call_signature',TSConstructSignatureDeclaration:'construct_signature',TSIndexSignature:'index_signature',TSFunctionType:'function_type',TSConstructorType:'constructor_type',TSEnumMember:'enum_member',TSImportEqualsDeclaration:'import_equals_declaration',TSExportAssignment:'export_assignment',ExpressionStatement:'expression_statement',ReturnStatement:'return_statement',ThrowStatement:'throw_statement',ImportDeclaration:'import_declaration',ExportNamedDeclaration:'export_named_declaration',ExportDefaultDeclaration:'export_default_declaration'};
const cases=[];const refsCases=[];const definitionsCases=[];const contextsCases=[];
for(const input of inputs){
 const path=join(root,'typed/artifacts',input.file);const bytes=readFileSync(path),source=bytes.toString('utf8');
 const typed=!input.file.endsWith('.jsx');const dts=input.file.endsWith('.d.ts');
 const ast=parse(source,{sourceType:'unambiguous',plugins:[...(typed?[['typescript',{dts}]]:[]),...(/\.tsx$|\.jsx$/.test(input.file)?['jsx']:[])]}).program;
 const byteCache=new Map();const byte=n=>{if(!byteCache.has(n))byteCache.set(n,Buffer.byteLength(source.slice(0,n)));return byteCache.get(n)};
 const range=n=>({start:byte(n.start),end:byte(n.end)});
 const identifier=n=>{
  // Use a greedy identifier token independent of any trailing TS annotation.
  const token=source.slice(n.start).match(/^(?:[$_\p{ID_Start}]|\\u(?:[\da-fA-F]{4}|\{[\da-fA-F]+\}))(?:[$\u200c\u200d\p{ID_Continue}]|\\u(?:[\da-fA-F]{4}|\{[\da-fA-F]+\}))*/u)[0];return{start:byte(n.start),end:byte(n.start+token.length)}};
 const entities=[],matches=[],anchors=new Map(),stack=[];let refQuery=null,definition=null;
 const entityRef=e=>e&&[e.kind,e.start,e.end];
 const name=(key,computed)=>computed?null:key?.type==='PrivateName'?`#${key.id.name}`:key?.name??(key?.value===undefined?null:String(key.value));
 const functionKinds=new Set(['FunctionDeclaration','FunctionExpression','ArrowFunctionExpression','ClassMethod','ClassPrivateMethod','TSDeclareMethod','TSDeclareFunction','TSMethodSignature']);
 function walk(node,parent=null,field=null,context=null,typeContext=false){
  if(!node||typeof node.type!=='string')return;
  let kind=null,entityName=null,flags={async:false,generator:false,static:false};
  if(node.type==='Program')kind='program';
  else if(['FunctionDeclaration','TSDeclareFunction','FunctionExpression'].includes(node.type)){kind=node.type==='FunctionExpression'?'function_expression':'function_declaration';entityName=node.id?.name??null;flags.async=!!node.async;flags.generator=!!node.generator;}
  else if(node.type==='ArrowFunctionExpression'){kind='arrow_function';flags.async=!!node.async;}
  else if(['ClassDeclaration','ClassExpression'].includes(node.type)){kind=node.type==='ClassDeclaration'?'class_declaration':'class_expression';entityName=node.id?.name??null;}
  else if(['ClassMethod','ClassPrivateMethod','TSDeclareMethod'].includes(node.type)){kind={constructor:'constructor',method:'class_method',get:'getter',set:'setter'}[node.kind];entityName=name(node.key,node.computed);flags={async:!!node.async,generator:!!node.generator,static:!!node.static};}
  else if(node.type==='ObjectMethod'){kind={method:'object_method',get:'getter',set:'setter'}[node.kind];entityName=name(node.key,node.computed);flags.async=!!node.async;flags.generator=!!node.generator;}
  else if(node.type==='TSTypeAliasDeclaration'){kind='type_alias';entityName=node.id.name;}
  else if(node.type==='TSInterfaceDeclaration'){kind='interface';entityName=node.id.name;}
  else if(node.type==='TSEnumDeclaration'){kind='enum';entityName=node.id.name;}
  else if(node.type==='TSModuleDeclaration'){kind=node.global?'global_declaration':node.id.type==='StringLiteral'?'external_module':'namespace';entityName=node.id.name??node.id.value;}
  else if(node.type==='TSMethodSignature'){kind='method_signature';entityName=name(node.key,node.computed);}
  if(kind){const e={kind,...(kind==='program'?{start:0,end:bytes.length}:range(node)),parent:entityRef(stack.at(-1))??null,depth:stack.length,name:entityName,flags};entities.push(e);stack.push(e);context=null;}
  else{let c=contextKinds[node.type];if(field==='params'&&functionKinds.has(parent?.type))c=node.type==='RestElement'?'rest_parameter':'parameter';if(node.type==='ExpressionStatement'&&node.directive)c=null;
   if(c)context={kind:c,...range(node),parent:context&&{kind:context.kind,start:context.start,end:context.end}};}
  let value=null,literalKind=null,dynamic=false;
  if(node.type==='StringLiteral'){value=node.value;literalKind=parent?.type==='JSXAttribute'?'jsx_attribute':parent?.type==='TSLiteralType'?'type_string':field==='key'?'property_key':'string';}
  if(node.type==='JSXText'){value=node.value;literalKind='jsx_text';}
  if(node.type==='TemplateElement'){value=node.value.cooked;literalKind=typeContext?'type_template':'template';dynamic=parent.expressions.length!==0;}
  if(value!==null&&value!==undefined&&value.includes(input.needle))matches.push({literal_kind:literalKind,value,...range(node),dynamic,entity:entityRef(stack.at(-1)),context});
  if(node.type==='Identifier')anchors.set(`${identifier(node).start}:${identifier(node).end}`,context);
  if(!definition&&node.type==='VariableDeclarator'&&node.id.type==='Identifier'&&node.init){definition={binding_name:node.id.name,identifier:identifier(node.id),declaration:{kind:'variable_declarator',...range(node),variable_kind:parent.kind,initializer:range(node.init)}};}
  if(!refQuery&&functionKinds.has(node.type)&&node.params){
   const parameter=node.params.find(p=>p.type==='Identifier');
   if(parameter){const param=identifier(parameter);const references=[];let valid=true;
    function uses(n,p=null,k=null,inType=false){if(!n||typeof n.type!=='string')return;
     if(['AssignmentExpression','UpdateExpression'].includes(n.type)||(n.type==='UnaryExpression'&&n.operator==='delete'))valid=false;
     if(n!==node&&functionKinds.has(n.type)&&(n.params??[]).some(q=>q.name===parameter.name))valid=false;
     if(n.type==='VariableDeclarator'&&n.id?.name===parameter.name)valid=false;
     if(n.type==='Identifier'&&n.name===parameter.name&&n.start!==parameter.start){
      const staticKey=(['MemberExpression','OptionalMemberExpression'].includes(p?.type)&&k==='property'&&!p.computed)||(['ObjectProperty','ObjectMethod','TSPropertySignature','TSMethodSignature'].includes(p?.type)&&k==='key'&&!p.computed);
      if(!staticKey&&!['id','params'].includes(k))references.push({...identifier(n),read:!inType,write:false,member_write:false,...(typed?{type_reference:inType,value_reference:!inType}:{})});
     }
     for(const[key,child]of Object.entries(n)){const t=inType||['typeAnnotation','returnType','typeParameters','typeArguments','constraint'].includes(key);if(Array.isArray(child))child.forEach(v=>uses(v,n,key,t));else if(child&&typeof child==='object')uses(child,n,key,t)}
    } uses(node);
    if(valid)refQuery={identifier:parameter.name,byte:param.start,declaration:param,entity:{kind:stack.at(-1).kind,start:stack.at(-1).start,end:stack.at(-1).end},references:references.sort((a,b)=>a.start-b.start||a.end-b.end)};
   }
  }
  for(const[key,child]of Object.entries(node)){const t=typeContext||key==='typeAnnotation'||key==='returnType'||node.type==='TSLiteralType';if(Array.isArray(child))child.forEach(v=>walk(v,node,key,context,t));else if(child&&typeof child==='object')walk(child,node,key,context,t)}
  if(kind)stack.pop();
 }
 walk(ast);
 entities.sort((a,b)=>a.start-b.start||b.end-a.end||(a.kind<b.kind?-1:a.kind>b.kind?1:0));matches.sort((a,b)=>a.start-b.start||a.end-b.end);
 const rows=entities.map(e=>[e.kind,e.start,e.end,e.parent,e.depth,e.name,e.flags.async,e.flags.generator,e.flags.static]);
 const sourceId=hash(bytes);const stem={name:input.name,file:`artifacts/${input.file}`,source_id:sourceId};
 let provenance={kind:'handcrafted',generator:'repository-authored parse-only source'};
 if(input.pkg){const pkg=lock.packages[`node_modules/${input.pkg}`];provenance={kind:'production',package:input.pkg,version:pkg.version,url:pkg.resolved,integrity:pkg.integrity,original_path:input.path,license:'MIT',license_file:`../licenses/${input.pkg.split('/').at(-1)}-types-MIT.txt`};}
 cases.push({name:input.name,file:stem.file,sha256:sourceId,input_bytes:bytes.length,needle:input.needle,plaintext_find:true,provenance,expected:{entity_count:entities.length,structure_sha256:hash(Buffer.from(JSON.stringify(rows))),matches:matches.map(({context,...m})=>m),sample_entities:entities.filter(e=>e.kind!=='program').slice(0,5)}});
 if(!refQuery)throw new Error(`No restricted parameter query ${input.name}`);
 refsCases.push({...stem,resolution:'resolved',...refQuery});
 if(definition)definitionsCases.push({...stem,...definition});
 const lookup=p=>({...p,context:anchors.get(`${p.start}:${p.end}`)??null});
 contextsCases.push({...stem,matches:matches.map(({start,end,context})=>({start,end,context})),selected:lookup(refQuery.declaration),references:refQuery.references.map(({start,end})=>lookup({start,end}))});
}
const generator={file:'../generate-typed.mjs',sha256:hash(readFileSync(fileURLToPath(import.meta.url))),babel:version,node:process.version};
writeFileSync(join(root,'typed/manifest.json'),JSON.stringify({corpus_version:1,generator,cases},null,2)+'\n');
writeFileSync(join(root,'typed/refs-expectations.json'),JSON.stringify({refs_version:1,generator,cases:refsCases},null,2)+'\n');
writeFileSync(join(root,'typed/definitions-expectations.json'),JSON.stringify({definitions_version:1,generator,cases:definitionsCases},null,2)+'\n');
writeFileSync(join(root,'typed/contexts-expectations.json'),JSON.stringify({contexts_version:1,generator,cases:contextsCases},null,2)+'\n');
console.log(`Wrote ${cases.length} independent typed/JSX source cases.`);
