use super::*;

#[test]
fn decoder_proofs_reject_mutation_escapes_fractional_indices_and_wrong_shapes() {
    let table = "function t(){const a=['needle','other'];t=function(){return a;};return t();}function d(i){i=i-16;const a=t();return a[i];}";
    for tail in [
        "const alias=d;alias=other;alias(16);",
        "const alias=d;t().reverse();alias(16);",
        "const alias=d;alias(16.5);alias('16junk');",
        "const alias=d;eval('d=other');alias(16);",
        "function invoke(d){return d(16)}",
        "const alias=d;alias('16');alias('0x10');alias('16junk');",
        "const alias=d;export {alias};alias(16);",
        "before(16);const before=d;",
    ] {
        let (_dir, path) = fixture(format!("{table}{tail}"), "unsafe.js");
        let rows = jsonl(&invoke(
            "find",
            &path,
            &["-f", "needle", "--resolve-static"],
        ));
        assert!(
            rows.iter().all(|r| r["detail"]["resolution"] != "decoder"),
            "{tail}: {rows:?}"
        );
    }
    let (_dir, path) = fixture(
        "function t(){const a=['needle','other'];return a;}function d(i){sideEffect();return t()[i];}d(0);",
        "effect.js",
    );
    let rows = jsonl(&invoke(
        "find",
        &path,
        &["-f", "needle", "--resolve-static"],
    ));
    assert!(rows.iter().all(|r| r["detail"]["resolution"] != "decoder"));
}

#[test]
fn decoder_values_resources_and_entity_labels_share_the_same_proof() {
    let source = "function t(){const a=['login','/api/login'];return a;}function d(i){return t()[i];}const o={[d(0)](){fetch(d(1));}};";
    let (_dir, path) = fixture(source, "shared.js");
    assert!(jsonl(&invoke("methods", &path, &["-f", "login"])).is_empty());
    let methods = jsonl(&invoke(
        "methods",
        &path,
        &["-f", "login", "--resolve-static"],
    ));
    assert_eq!(methods.len(), 1, "{methods:?}");
    assert_eq!(methods[0]["detail"]["label"], "login");
    assert_eq!(methods[0]["detail"]["label_origin"], "decoder");
    assert!(methods[0]["detail"]["name"].is_null());
    let values = jsonl(&invoke(
        "values",
        &path,
        &["--kind", "property", "--name", "login", "--resolve-static"],
    ));
    assert_eq!(values.len(), 1, "{values:?}");
    assert_eq!(values[0]["detail"]["key_resolution"], "decoder");
    let resources = jsonl(&invoke(
        "resources",
        &path,
        &["-f", "/api/login", "--resolve-static"],
    ));
    assert_eq!(resources.len(), 1, "{resources:?}");
    assert_eq!(resources[0]["detail"]["resolution"], "decoder");
    let sliced = invoke(
        "slice",
        &path,
        &["--at", methods[0]["at"].as_str().unwrap()],
    );
    success(&sliced);
    let start = methods[0]["start"].as_u64().unwrap() as usize;
    let end = methods[0]["end"].as_u64().unwrap() as usize;
    assert_eq!(sliced.stdout, source.as_bytes()[start..end]);
}

#[test]
fn base64_table_recovery_preserves_atob_latin1_and_rejects_partial_tables() {
    for (array, expected) in [
        ("['bG9naW4=','/w==']", true),
        ("['bG9naW4=','invalid!']", false),
    ] {
        let source = format!(
            "function t(){{const a={array};return a;}}function d(i){{return atob(t()[i]);}}const x=d(0),y=d(1);"
        );
        let (_dir, path) = fixture(source, "base64.js");
        let rows = jsonl(&invoke("find", &path, &["-f", "login", "--resolve-static"]));
        assert_eq!(!rows.is_empty(), expected, "{rows:?}");
        if expected {
            let latin = jsonl(&invoke(
                "find",
                &path,
                &["-f", "ÿ", "--case", "sensitive", "--resolve-static"],
            ));
            assert_eq!(latin.len(), 1, "{latin:?}");
        }
    }
}

#[test]
fn decoder_intrinsics_initialization_and_utf16_follow_source_semantics() {
    let prefix = "function t(){const a=['bG9naW4='];return a;}function d(i){return atob(t()[i]);}";
    for tail in [
        "globalThis.atob=other;d(0);",
        "globalThis[\'at\'+\'ob\']=other;d(0);",
        "const g=globalThis;const name=\'atob\';g[name]=other;d(0);",
        "delete globalThis.atob;d(0);",
        "Object.defineProperty(globalThis,'atob',{value:other});d(0);",
        "const alias=atob;mutate(alias);d(0);",
        "function atob(s){return 'wrong'}d(0);",
    ] {
        let (_dir, path) = fixture(format!("{prefix}{tail}"), "intrinsic.js");
        let rows = jsonl(&invoke("find", &path, &["-f", "login", "--resolve-static"]));
        assert!(
            rows.iter().all(|r| r["detail"]["resolution"] != "decoder"),
            "{tail}: {rows:?}"
        );
    }
    let (_dir, path) = fixture(
        "function t(){const t=['needle'];t=function(){return t};return t()}function d(i){return t()[i]}d(0);",
        "shadow.js",
    );
    let rows = jsonl(&invoke(
        "find",
        &path,
        &["-f", "needle", "--resolve-static"],
    ));
    assert!(
        rows.iter().all(|r| r["detail"]["resolution"] != "decoder"),
        "{rows:?}"
    );
    let source =
        r#"function t(){const a=['\uD800needle'];return a}function d(i){return t()[i]}d(0);"#;
    let (_dir, path) = fixture(source, "surrogate.js");
    let output = invoke("find", &path, &["-f", "needle", "--resolve-static"]);
    success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.lines()
            .any(|line| line.contains("\"resolution\":\"decoder\"")
                && line.contains("\\ud800\\u006e\\u0065\\u0065\\u0064\\u006c\\u0065")),
        "{text}"
    );
    let source = r#"function t(){const a=['\u00852','pad','needle'];t=function(){return a};return t()}function d(i){return t()[i]}(function(f,n){const a=f();while(true){try{const v=parseInt(d(0));if(v===n)break;else a.push(a.shift());}catch(e){a.push(a.shift())}}})(t,2);d(2);"#;
    let (_dir, path) = fixture(source, "whitespace.js");
    let rows = jsonl(&invoke(
        "find",
        &path,
        &["-f", "needle", "--resolve-static"],
    ));
    assert!(
        rows.iter().all(|r| r["detail"]["resolution"] != "decoder"),
        "{rows:?}"
    );
}

#[test]
fn rc4_is_explicitly_unresolved_in_default_and_static_queries() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/artifacts/application-rc4.js");
    for options in [
        vec!["-f", "/api/login"],
        vec!["-f", "/api/login", "--resolve-static"],
    ] {
        assert!(jsonl(&invoke("find", &path, &options)).is_empty());
    }
}

#[test]
fn module_imports_exports_keep_aliases_types_unicode_and_original_navigation() {
    let source = "import main, {type T, login as signIn} from './auth.mjs';import './side.mjs';export {signIn as 登录};export {login as remote} from './auth.mjs';export * from './other.mjs';export const n=1;export default function run(){return signIn();}";
    let (_dir, path) = fixture(source, "entry.ts");
    let imports = jsonl(&invoke("imports", &path, &[]));
    assert_eq!(imports.len(), 4, "{imports:?}");
    let login = imports
        .iter()
        .find(|r| r["detail"]["imported"] == "login")
        .unwrap();
    assert_eq!(login["detail"]["local"], "signIn");
    assert_eq!(login["detail"]["source"], "./auth.mjs");
    success(&invoke(
        "slice",
        &path,
        &["--at", login["at"].as_str().unwrap()],
    ));
    let types = imports
        .iter()
        .find(|r| r["detail"]["imported"] == "T")
        .unwrap();
    assert_eq!(types["detail"]["type_only"], true);
    let exports = jsonl(&invoke("exports", &path, &[]));
    assert_eq!(exports.len(), 5, "{exports:?}");
    assert!(
        exports
            .iter()
            .any(|r| r["detail"]["exported"] == "登录" && r["at"].is_string())
    );
    assert!(
        exports
            .iter()
            .any(|r| r["detail"]["exported"] == "remote" && r["detail"]["syntax"] == "reexport")
    );
    assert_eq!(
        jsonl(&invoke("exports", &path, &["--name", "default"])).len(),
        1
    );
    let modules = jsonl(&invoke("modules", &path, &[]));
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0]["detail"]["format"], "esm");
    assert!(modules[0]["detail"]["module_id"].is_null());
    assert_eq!(modules[0]["detail"]["imports"], 4);
}

#[test]
fn retained_webpack_registry_and_chunks_expose_modules_and_dependency_sites() {
    let source = "const table={42:(m,e,r)=>{const dep=r(7);e.login=dep;},7:(m)=>{m.exports=()=>1;}};const cache={};function req(id){const m={exports:{}};table[id](m,m.exports,req);return m.exports;}req(42);(self.webpackChunk_app=self.webpackChunk_app||[]).push([[3],{99:(m,e,r)=>{r.d(e,{name:()=>value});r(42);r(dynamic);}}]);";
    let (_dir, path) = fixture(source, "bundle.js");
    let modules = jsonl(&invoke("modules", &path, &[]));
    let bundle: Vec<_> = modules
        .iter()
        .filter(|r| r["detail"]["format"] == "webpack")
        .collect();
    assert_eq!(bundle.len(), 3, "{modules:?}");
    let target = bundle
        .iter()
        .find(|r| r["detail"]["module_id"] == "42")
        .unwrap();
    assert_eq!(target["detail"]["entry"], true);
    let imports = jsonl(&invoke(
        "imports",
        &path,
        &["--within", target["at"].as_str().unwrap()],
    ));
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(imports[0]["detail"]["source"], "7");
    let exports = jsonl(&invoke("exports", &path, &[]));
    assert!(exports.iter().any(|r| r["detail"]["exported"] == "login"));
    assert!(exports.iter().any(|r|r["detail"]["exported"]=="name"&&r["detail"]["syntax"]=="webpack_getter"));
    assert!(
        jsonl(&invoke("imports", &path, &[]))
            .iter()
            .any(|r| r["detail"]["expression"] == "dynamic")
    );
    for module in bundle {
        let sliced = invoke("slice", &path, &["--at", module["at"].as_str().unwrap()]);
        success(&sliced);
        let start = module["start"].as_u64().unwrap() as usize;
        let end = module["end"].as_u64().unwrap() as usize;
        assert_eq!(sliced.stdout, source.as_bytes()[start..end]);
    }
}

#[test]
fn published_vsix_modules_match_independent_acorn_ranges_and_original_bytes() {
    use std::io::Read;
    let oracle: Value =
        serde_json::from_str(include_str!("../corpus/modules-expectations.json")).unwrap();
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(include_bytes!("../corpus/generate-modules.mjs"))
        ),
        oracle["generator"]["sha256"]
    );
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/zip");
    let archive = fs::read(corpus.join(oracle["archive"].as_str().unwrap())).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&archive)),
        oracle["archive_sha256"]
    );
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
    let mut bytes = Vec::new();
    zip.by_name(oracle["entry"].as_str().unwrap())
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), oracle["sha256"]);
    let (_dir, path) = fixture(&bytes, "languageserver.js");
    let modules = jsonl(&invoke("modules", &path, &[]));
    let mut modules: Vec<_> = modules
        .iter()
        .filter(|r| r["detail"]["format"] == "webpack")
        .collect();
    modules.sort_by_key(|r| r["start"].as_u64().unwrap());
    let expected = oracle["modules"].as_array().unwrap();
    assert_eq!(modules.len(), expected.len());
    for (actual, expected) in modules.iter().zip(expected) {
        assert_eq!(actual["detail"]["module_id"], expected["id"]);
        assert_eq!(actual["start"], expected["start"]);
        assert_eq!(actual["end"], expected["end"]);
        assert!(
            actual["at"]
                .as_str()
                .unwrap()
                .starts_with(&format!("{}:", expected["kind"].as_str().unwrap()))
        );
        let start = actual["start"].as_u64().unwrap() as usize;
        let end = actual["end"].as_u64().unwrap() as usize;
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes[start..end])),
            expected["sha256"]
        );
    }
    for index in [0, modules.len() / 2, modules.len() - 1] {
        let row = modules[index];
        let sliced = invoke("slice", &path, &["--at", row["at"].as_str().unwrap()]);
        success(&sliced);
        assert_eq!(
            sliced.stdout,
            bytes[row["start"].as_u64().unwrap() as usize..row["end"].as_u64().unwrap() as usize]
        );
    }
}

#[test]
fn registry_proofs_reject_callbacks_spreads_and_fake_chunk_receivers() {
    for source in [
        "const t={7:(m,e,r)=>{}};function r(id){t[id](1,2,r)}r(7)",
        "const t={7:(m,e,r)=>{}};function r(id){const m={exports:{}};t[id](m,m.exports,other)}r(7)",
        "const t=[...unknown,(m,e,r)=>{}];function r(id){const m={exports:{}};t[id](m,m.exports,r)}r(0)",
        "const t={7:(m,e,r)=>{},...unknown};function r(id){const m={exports:{}};t[id](m,m.exports,r)}r(7)",
        "const t={7:(m,e,r)=>{},7:1};function r(id){const m={exports:{}};t[id](m,m.exports,r)}r(7)",
        "const o={webpackChunkFoo:[]};o.webpackChunkFoo.push([[1],{7:(m,e,r)=>{}}]);",
        "(self.webpackChunkFoo=unknown).push([[1],{7:(m,e,r)=>{}}]);",
    ] {
        let (_dir, path) = fixture(source, "not-a-bundle.js");
        let rows = jsonl(&invoke("modules", &path, &[]));
        assert_eq!(rows.len(), 1, "{source}: {rows:?}");
    }
    let source = "const t={7:(m,e,r)=>{r=other;e={};r(8);e.fake=1;}};function r(id){const m={exports:{}};t[id](m,m.exports,r)}function later(){r(7)}if(false)r(7);";
    let (_dir, path) = fixture(source, "deferred.js");
    let rows = jsonl(&invoke("modules", &path, &[]));
    assert_eq!(rows.len(), 2);
    assert!(rows[1]["detail"]["entry"].is_null());
    assert!(jsonl(&invoke("imports", &path, &[])).is_empty());
    assert!(jsonl(&invoke("exports", &path, &[])).is_empty());
    let (_dir, path) = fixture(
        r#"import {x as y} from './\uD800.mjs';export {y as '普通'};"#,
        "surrogate.mjs",
    );
    let output = invoke("imports", &path, &[]);
    success(&output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("\\ud800")
    );
    let (_dir, path) = fixture(r#"const y=1;export {y as '\uD800'};"#, "invalid.mjs");
    failure(invoke("exports", &path, &[]), 1, "INVALID_INPUT");
}

#[test]
fn commonjs_listings_preserve_definition_evidence_and_ignore_shadowed_roles() {
    let source = "const local=1;module.exports={login:local,plain(){}};exports.remote=local;const x=require('./a.cjs');function f(require,exports,module){require('fake');exports.fake=1;module.exports.fake=1;}";
    let (_dir, path) = fixture(source, "entry.cjs");
    let exports = jsonl(&invoke("exports", &path, &[]));
    assert_eq!(exports.len(), 4, "{exports:?}");
    assert!(
        exports
            .iter()
            .any(|r| r["detail"]["exported"] == "login" && r["at"].is_string())
    );
    assert!(exports.iter().all(|r| r["detail"]["exported"] != "fake"));
    let imports = jsonl(&invoke("imports", &path, &[]));
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(imports[0]["detail"]["source"], "./a.cjs");
    let (_dir, path) = fixture("require(7);", "number.cjs");
    let imports = jsonl(&invoke("imports", &path, &[]));
    assert!(imports[0]["detail"]["source"].is_null());
    assert_eq!(imports[0]["detail"]["expression"], "7");
    let (_dir, path) = fixture("export const name=1;module.exports=name;", "mixed.mjs");
    assert_eq!(
        jsonl(&invoke("modules", &path, &[]))[0]["detail"]["format"],
        "mixed"
    );
    let (_dir, path) = fixture(
        "require=other;require('./fake.cjs');exports=other;exports.fake=1;module=other;module.exports.fake=1;",
        "written.cjs",
    );
    assert!(jsonl(&invoke("imports", &path, &[])).is_empty());
    assert!(jsonl(&invoke("exports", &path, &[])).is_empty());
}

#[test]
fn review_decoder_proofs_reject_early_capture_exports_and_prototype_mutation() {
    let plain = "function t(){const a=['needle','other'];return a;}function d(i){return t()[i]}";
    let cached = "function t(){const a=['2','1','needle'];t=function(){return a};return t()}function d(i){return t()[i]}";
    let rotation = "(function(f,n){const a=f();while(true){try{const v=parseInt(d(0));if(v===n)break;else a.push(a.shift())}catch(e){a.push(a.shift())}}})(t,1);";
    let sources=[
        format!("{plain}f();const a=d;function f(){{return a(0)}}"),
        format!("{plain}globalThis.f();const a=d;function f(){{return a(0)}}"),
        format!("{plain}this.f();const a=d;function f(){{return a(0)}}"),
        format!("{cached}globalThis.t()[2]=\'changed\';d(2);"),
        format!("{cached}globalThis.d=()=>\'changed\';d(2);"),
        format!("{plain}(0,eval)(\"d=()=>\'changed\'\");d(0);"),
        format!("{plain}const a=d;export function f(){{return a(0)}}"),
        format!("{plain}const a=d;function f(){{return a(0)}}export {{f}};"),
        format!("{plain}switch(1){{case 0:const a=d;break;case 1:a(0);}}"),
        format!("{cached}f();{rotation}function f(){{return d(1)}}"),
        format!("export {cached}export function read(){{return d(2)}}"),
        format!("[].__proto__.shift=function(){{this[0]='1';return 'x'}};{cached}{rotation}d(1);"),
        "Object.getPrototypeOf([]).push=function(x){this[0]='Mg==';this[1]='V1JPTkc=';};function t(){const a=['cGFk','Mg==','bmVlZGxl'];t=function(){return a};return t()}function d(i){return atob(t()[i])}(function(f,n){const a=f();while(true){try{const v=parseInt(d(0));if(v===n)break;else a.push(a.shift())}catch(e){a.push(a.shift())}}})(t,2);d(1);".to_owned(),
    ];
    let mut false_proofs = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let (_dir, path) = fixture(source, "proof.js");
        let rows = jsonl(&invoke(
            "find",
            &path,
            &["-f", "needle", "--resolve-static"],
        ));
        if rows.iter().any(|r| r["detail"]["resolution"] == "decoder") {
            false_proofs.push(index);
        }
    }
    assert!(
        false_proofs.is_empty(),
        "false decoder evidence in cases {false_proofs:?}"
    );
    let (_dir, path) = fixture(
        format!("{plain}const a=d;f();function f(){{return a(0)}}"),
        "ready.js",
    );
    assert!(
        jsonl(&invoke(
            "find",
            &path,
            &["-f", "needle", "--resolve-static"]
        ))
        .iter()
        .any(|r| r["detail"]["resolution"] == "decoder")
    );
}

#[test]
fn review_second_round_rejects_ambient_this_evaluators_exports_and_opaque_bootstrap() {
    let plain = "function t(){const a=['needle'];t=function(){return a};return t()}function d(i){return t()[i]}";
    let encoded = "function t(){const a=['bmVlZGxl'];t=function(){return a};return t()}function d(i){return atob(t()[i])}";
    let cached = "function t(){const a=['2','1','needle'];t=function(){return a};return t()}function d(i){return t()[i]}";
    let rotate = "(function(f,n){const a=f();while(true){try{const v=parseInt(d(0));if(v===n)break;else a.push(a.shift())}catch(e){a.push(a.shift())}}})(t,1);d(1);";
    let sources = [
        format!("{encoded}const g=this;g.t()[0]='V1JPTkc=';d(0);"),
        format!("{plain}const g=this;g.f();const a=d;function f(){{return a(0)}}"),
        format!("{encoded}globalThis.eval(\"d=()=> 'WRONG'\");d(0);"),
        format!("{encoded}globalThis['ev'+'al'](\"d=()=> 'WRONG'\");d(0);"),
        format!("{encoded}const k='eval';globalThis[k](\"d=()=> 'WRONG'\");d(0);"),
        format!(
            "const k='prototype',p=[].constructor[k];Object.defineProperty(p,'shift',{{value:function(){{return 'x'}}}});{cached}{rotate}"
        ),
        format!(
            "const p=Reflect.get([],'__proto__');Object.defineProperty(p,'shift',{{value:function(){{return 'x'}}}});{cached}{rotate}"
        ),
        format!("{cached}opaqueBootstrap();{rotate}"),
        "function t(){const a=['needle'];return a}export function d(i){return t()[i]}d(0);"
            .to_owned(),
        format!("{plain}export const a=d;a(0);"),
        format!("{plain}const k='constructor',C=(0,(()=>{{}})[k]);C(\"d=()=> 'WRONG'\")();d(0);"),
        format!("{plain}const k='constructor',[C]=[(()=>{{}})[k]];C(\"d=()=> 'WRONG'\")();d(0);"),
        format!("import './opaque.mjs';{encoded}d(0);"),
        format!("{plain}const {{constructor:C}}=()=>{{}};C(\"d=()=> 'WRONG'\")();d(0);"),
        format!("{plain}const k='constructor',c=(()=>{{}})[k];c(\"d=()=> 'WRONG'\")();d(0);"),
    ];
    let mut false_proofs = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let (_dir, path) = fixture(source, "ambient.js");
        if jsonl(&invoke(
            "find",
            &path,
            &["-f", "needle", "--resolve-static"],
        ))
        .iter()
        .any(|r| r["detail"]["resolution"] == "decoder")
        {
            false_proofs.push(index);
        }
    }
    assert!(
        false_proofs.is_empty(),
        "false decoder evidence in ambient cases {false_proofs:?}"
    );
}

#[test]
fn review_closes_global_reentry_patterns_and_preserves_independent_tables() {
    let encoded = "function t(){const a=['bmVlZGxl'];t=function(){return a};return t()}function d(i){return atob(t()[i])}";
    for receiver in [
        "globalThis.globalThis",
        "globalThis['global'+'This']",
        "this.globalThis",
    ] {
        let (_dir, path) = fixture(
            format!("{encoded}{receiver}.t()[0]='V1JPTkc=';d(0);"),
            "reentry.js",
        );
        assert!(
            jsonl(&invoke(
                "find",
                &path,
                &["-f", "needle", "--resolve-static"]
            ))
            .iter()
            .all(|r| r["detail"]["resolution"] != "decoder"),
            "{receiver}"
        );
    }
    let t = "function t(){const a=['2','1','needle'];t=function(){return a};return t()}function d(i){return t()[i]}";
    let u = "function u(){const a=['2','1','other'];u=function(){return a};return u()}function e(i){return u()[i]}";
    let first = "(function(f,n){const a=f();while(true){try{const v=parseInt(d(0));if(v===n)break;else a.push(a.shift())}catch(e){a.push(a.shift())}}})(t,1);";
    let second = "(function(f,n){const a=f();while(true){try{const v=parseInt(e(0));if(v===n)break;else a.push(a.shift())}catch(e){a.push(a.shift())}}})(u,1);";
    for source in [
        "function t(){const a=['needle'];return a}function d(i){return t()[i]}function u(){const a=['other'];return a}function e(i){return u()[i]}d(0);e(0);client[d(0)]();client[e(0)]();".to_owned(),
        format!("{t}{u}{first}{second}d(1);e(1);client[d(1)]();client[e(1)]();"),
    ] {
        let (_dir,path)=fixture(source,"siblings.js");
        for needle in ["needle","other"] {
            let rows=jsonl(&invoke("find",&path,&["-f",needle,"--resolve-static"]));
            assert_eq!(rows.iter().filter(|r|r["detail"]["resolution"]=="decoder").count(),2,"{needle}: {rows:?}");
            assert_eq!(jsonl(&invoke("members",&path,&["--name",needle,"--resolve-static"])).len(),1);
        }
    }
    let (_dir, path) = fixture(
        format!("const [unused=opaqueBootstrap()]=[];{t}{first}d(1);"),
        "pattern.js",
    );
    assert!(
        jsonl(&invoke(
            "find",
            &path,
            &["-f", "needle", "--resolve-static"]
        ))
        .iter()
        .all(|r| r["detail"]["resolution"] != "decoder")
    );
}

#[test]
fn review_project_relative_parent_paths_and_mutated_entry_selectors_are_not_guessed() {
    let source = "const t={7:(m,e,r)=>{},8:(m,e,r)=>{}};function r(id){id++;const m={exports:{}};t[id](m,m.exports,r)}r(7)";
    let (_dir, path) = fixture(source, "selector.js");
    let modules = jsonl(&invoke("modules", &path, &[]));
    assert_eq!(modules.len(), 3);
    assert!(
        modules.iter().all(|r| r["detail"]["entry"].is_null()),
        "{modules:?}"
    );
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/auth.mjs"),
        "export function value(){};value();",
    )
    .unwrap();
    for (source, expected) in [("../../src/auth.mjs", 1), ("./auth.mjs", 2)] {
        fs::write(
            dir.path().join("src/use.mjs"),
            format!("import {{value}} from '{source}';value();"),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .current_dir(dir.path())
            .args(["js", "xref", "src", "--file", "auth.mjs", "--byte", "16"])
            .output()
            .unwrap();
        assert_eq!(
            jsonl(&output)
                .iter()
                .filter(|r| r["record"] == "reference")
                .count(),
            expected,
            "{source}"
        );
    }
}

#[test]
fn directory_searches_are_deterministic_local_and_atomic() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("a.mjs"),
        "export function login(){};login();const x={login:1};",
    )
    .unwrap();
    fs::write(
        dir.path().join("b.mjs"),
        "function login(){};login();const x={login:2};",
    )
    .unwrap();
    for (command, args) in [
        ("calls", vec!["--name", "login"]),
        ("members", vec!["--name", "login"]),
        ("identifiers", vec!["--name", "login"]),
        ("symbols", vec!["--name", "login"]),
        ("xref", vec!["--kind", "symbol", "--match", "login"]),
        ("imports", vec![]),
        ("exports", vec![]),
        ("modules", vec![]),
    ] {
        let mut serial = args.clone();
        serial.extend(["--threads", "1"]);
        let mut parallel = args.clone();
        parallel.extend(["--threads", "2"]);
        let one = invoke(command, dir.path(), &serial);
        let two = invoke(command, dir.path(), &parallel);
        success(&one);
        success(&two);
        assert_eq!(one.stdout, two.stdout, "{command}");
    }
    assert!(
        !invoke("calls", dir.path(), &["--within", "program:0:3"])
            .status
            .success()
    );
    fs::write(dir.path().join("z.mjs"), "invalid { {{").unwrap();
    let failed = invoke("calls", dir.path(), &["--name", "login"]);
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
}

#[test]
fn project_graph_distinguishes_equivalent_paths_ambiguity_missing_targets_and_cycles() {
    for (bridge, linked) in [
        (
            "export * from './auth.mjs';export * from './same.mjs';",
            true,
        ),
        (
            "export * from './loop.mjs';export * from './auth.mjs';",
            true,
        ),
        (
            "export {value} from './auth.mjs';export * from './other.mjs';",
            true,
        ),
        (
            "export * from './auth.mjs';export * from './other.mjs';",
            false,
        ),
        (
            "export * from './auth.mjs';export * from './missing.mjs';",
            false,
        ),
        ("export {value} from './auth';", false),
        ("export * as value from './auth.mjs';", false),
        ("export {value} from './loop.mjs';", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        for (file, source) in [
            ("auth.mjs", "export function value(){};value();"),
            ("other.mjs", "export function value(){};value();"),
            ("same.mjs", "export {value} from './auth.mjs';"),
            ("loop.mjs", "export * from './bridge.mjs';"),
            ("bridge.mjs", bridge),
            (
                "client.mjs",
                "import {value as use} from './bridge.mjs';use();",
            ),
        ] {
            fs::write(dir.path().join(file), source).unwrap();
        }
        let result = invoke("xref", dir.path(), &["--file", "auth.mjs", "--byte", "16"]);
        let rows = jsonl(&result);
        let references: Vec<_> = rows.iter().filter(|r| r["record"] == "reference").collect();
        assert_eq!(
            references.len(),
            if linked { 2 } else { 1 },
            "{bridge}: {rows:?}"
        );
        assert_eq!(
            references
                .iter()
                .filter(|r| r["detail"]["link"] == "esm")
                .count(),
            usize::from(linked),
            "{bridge}: {rows:?}"
        );
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("auth.mjs"),
        "export function value(){};value();",
    )
    .unwrap();
    for index in (0..40).rev() {
        let source = if index == 39 {
            "./auth.mjs".to_owned()
        } else {
            format!("./link{}.mjs", index + 1)
        };
        fs::write(
            dir.path().join(format!("link{index}.mjs")),
            format!("export * from '{source}';"),
        )
        .unwrap();
    }
    fs::write(
        dir.path().join("client.mjs"),
        "import {value} from './link0.mjs';value();",
    )
    .unwrap();
    let rows = jsonl(&invoke(
        "xref",
        dir.path(),
        &["--file", "auth.mjs", "--byte", "16"],
    ));
    assert_eq!(
        rows.iter().filter(|r| r["record"] == "reference").count(),
        2
    );
}

#[test]
fn project_defaults_type_roles_and_member_writes_keep_their_actual_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let auth = dir.path().join("auth.mjs");
    fs::write(&auth, "let item=1;export default item;item=2;").unwrap();
    fs::write(
        dir.path().join("client.mjs"),
        "import alias from './auth.mjs';alias;",
    )
    .unwrap();
    let rows = jsonl(&invoke(
        "xref",
        dir.path(),
        &["--file", "auth.mjs", "--byte", "4"],
    ));
    assert_eq!(
        rows.iter().filter(|r| r["record"] == "reference").count(),
        2,
        "{rows:?}"
    );
    assert!(rows.iter().all(|r| r["detail"]["link"] != "esm"));
    fs::write(&auth, "export default function value(){};value();").unwrap();
    let a = jsonl(&invoke(
        "xref",
        dir.path(),
        &["--file", "auth.mjs", "--byte", "24"],
    ));
    let b = jsonl(&invoke(
        "xref",
        dir.path(),
        &["--file", "client.mjs", "--at", "identifier:7:12"],
    ));
    assert_eq!(a, b);
    assert_eq!(
        a.iter().filter(|r| r["record"] == "reference").count(),
        2,
        "{a:?}"
    );
    failure(
        invoke(
            "xref",
            dir.path(),
            &["--file", "client.mjs", "--at", "identifier:07:12"],
        ),
        1,
        "INVALID_QUERY",
    );
    fs::write(
        dir.path().join("types.ts"),
        "export interface T{n:number}export const data={n:1};",
    )
    .unwrap();
    fs::write(
        dir.path().join("use.ts"),
        "import type {T} from './types.ts';import {data} from './types.ts';let x:T;data.n=2;",
    )
    .unwrap();
    let types = jsonl(&invoke(
        "xref",
        dir.path(),
        &["--file", "types.ts", "--byte", "17"],
    ));
    assert!(
        types.iter().any(|r| r["detail"]["link"] == "esm"
            && r["detail"]["type_reference"] == true
            && r["detail"]["value_reference"] == false),
        "{types:?}"
    );
    let data = jsonl(&invoke(
        "xref",
        dir.path(),
        &["--file", "types.ts", "--byte", "41"],
    ));
    assert!(
        data.iter().any(|r| r["detail"]["link"] == "esm"
            && r["detail"]["member_write"] == true
            && r["detail"]["value_reference"] == true),
        "{data:?}"
    );
}

#[cfg(unix)]
#[test]
fn project_paths_never_follow_symlinks_outside_files_or_encoded_specifier_guesses() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir(&root).unwrap();
    fs::write(dir.path().join("outside.mjs"), "invalid {{ {").unwrap();
    fs::write(root.join("auth.mjs"), "export function value(){};value();").unwrap();
    fs::write(
        root.join("%61uth.mjs"),
        "export function value(){};value();",
    )
    .unwrap();
    fs::write(
        root.join("client.mjs"),
        "import {value} from './%61uth.mjs';value();import '../outside.mjs';",
    )
    .unwrap();
    symlink(root.join("auth.mjs"), root.join("link.mjs")).unwrap();
    let rows = jsonl(&invoke(
        "xref",
        &root,
        &["--file", "%61uth.mjs", "--byte", "16"],
    ));
    assert_eq!(
        rows.iter().filter(|r| r["record"] == "reference").count(),
        1,
        "{rows:?}"
    );
    failure(
        invoke("xref", &root, &["--file", "link.mjs", "--byte", "16"]),
        1,
        "NOT_FOUND",
    );
    failure(
        invoke("xref", &root, &["--file", "../outside.mjs", "--byte", "0"]),
        2,
        "INVALID_QUERY",
    );
}

#[test]
fn project_xref_follows_exact_esm_aliases_and_reexports_not_same_names() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("auth.mjs"),
        "export function login(){};login();",
    )
    .unwrap();
    fs::write(
        dir.path().join("bridge.mjs"),
        "export {login as enter} from './auth.mjs';",
    )
    .unwrap();
    fs::write(
        dir.path().join("client.mjs"),
        "import {enter as signIn} from './bridge.mjs';signIn();function f(signIn){signIn();}",
    )
    .unwrap();
    fs::write(
        dir.path().join("unrelated.mjs"),
        "export function login(){};login();",
    )
    .unwrap();
    let auth = dir.path().join("auth.mjs");
    let exported = jsonl(&invoke("exports", &auth, &["--name", "login"]));
    let at = exported[0]["at"].as_str().unwrap();
    let result = invoke("xref", dir.path(), &["--file", "auth.mjs", "--at", at]);
    success(&result);
    let records = jsonl(&result);
    let refs: Vec<_> = records
        .iter()
        .filter(|r| r["record"] == "reference")
        .collect();
    assert_eq!(refs.len(), 2, "{records:?}");
    assert!(refs.iter().any(|r| r["detail"]["link"] == "esm"));
    assert!(records.iter().filter(|r| r["record"] == "file").all(|r| {
        r["detail"]["path"]
            .as_str()
            .is_some_and(|p| !p.ends_with("unrelated.mjs"))
    }));
    assert!(!invoke("xref", dir.path(), &["--at", at]).status.success());
    assert!(
        !invoke("xref", dir.path(), &["--file", "../auth.mjs", "--at", at])
            .status
            .success()
    );
}
