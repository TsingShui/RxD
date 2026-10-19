//! Offline Java-layer hooks resolved against one immutable APK/DEX capture.
use crate::{Artifact, CapturedInput, Diagnostic, ErrorKind, Language, Limits, Result, Stage};

#[derive(Clone, Debug)]
pub enum Selector {
    Signature(String),
    MethodIndex { class: String, method_index: u32 },
}
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub language: Language,
    pub import_java_bridge: bool,
    /// Exact physical/logical DEX entry, not an ordinal across the archive.
    pub entry: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Request {
    pub input: CapturedInput,
    pub selector: Selector,
    pub options: Options,
}

pub fn generate(request: &Request, limits: Limits) -> Result<Artifact> {
    limits.validate()?;
    limits.check_input(request.input.bytes().len())?;
    #[cfg(feature = "dex")]
    {
        enabled::generate(request, limits)
    }
    #[cfg(not(feature = "dex"))]
    {
        Err(Diagnostic::new(
            ErrorKind::ProviderUnavailable,
            Stage::Selection,
            "DEX provider requires feature dex",
        ))
    }
}

#[cfg(feature = "dex")]
mod enabled {
    use super::*;
    use crate::{
        SourceArtifact,
        source::{SourceBuilder, StageEvent, js_string, setup_catch, setup_prelude, setup_stage},
    };
    use rxd_dex::hooks;

    fn diagnostic(kind: ErrorKind, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(kind, Stage::Selection, message)
    }
    fn invalid(message: &str) -> Diagnostic {
        diagnostic(ErrorKind::InvalidArguments, message)
    }

    // Descriptor grammar is bounded and iterative: no recursion on untrusted array ranks.
    fn descriptor(input: &str, allow_void: bool) -> Option<(&str, &str)> {
        let bytes = input.as_bytes();
        let rank = bytes.iter().take_while(|&&c| c == b'[').count();
        if rank > 255 {
            return None;
        }
        let end = match *bytes.get(rank)? {
            b'V' if allow_void && rank == 0 => rank + 1,
            b'Z' | b'B' | b'C' | b'S' | b'I' | b'J' | b'F' | b'D' => rank + 1,
            b'L' => {
                let end = input[rank + 1..].find(';')? + rank + 1;
                let name = &input[rank + 1..end];
                if name.is_empty()
                    || name.split('/').any(str::is_empty)
                    || name.chars().any(|c| c.is_control() || ".;[()".contains(c))
                {
                    return None;
                }
                end + 1
            }
            _ => return None,
        };
        Some(input.split_at(end))
    }
    fn prototype(input: &str) -> Result<(Vec<&str>, &str)> {
        let mut rest = input
            .strip_prefix('(')
            .ok_or_else(|| invalid("complete method prototype required: (PARAMETERS)RETURN"))?;
        let mut parameters = Vec::new();
        while !rest.starts_with(')') {
            let (parameter, tail) =
                descriptor(rest, false).ok_or_else(|| invalid("invalid parameter descriptor"))?;
            parameters.push(parameter);
            rest = tail;
        }
        rest = &rest[1..];
        let (result, tail) =
            descriptor(rest, true).ok_or_else(|| invalid("complete return descriptor required"))?;
        if !tail.is_empty() {
            return Err(invalid("unexpected text after return descriptor"));
        }
        Ok((parameters, result))
    }
    fn class_descriptor(input: &str) -> Result<String> {
        let class = if input.starts_with('L') && input.ends_with(';') {
            input.to_owned()
        } else {
            format!("L{};", input.replace('.', "/"))
        };
        if !class.starts_with('L')
            || descriptor(&class, false).is_none_or(|(_, tail)| !tail.is_empty())
        {
            return Err(invalid(
                "declaring class must be a binary class name or Lpkg/Class; descriptor",
            ));
        }
        Ok(class)
    }
    pub(super) fn generate(request: &Request, limits: Limits) -> Result<Artifact> {
        let (class, name_proto, index) = match &request.selector {
            Selector::Signature(signature) => {
                limits.check_target(signature)?;
                let (class, member) = signature.split_once("->").ok_or_else(|| {
                    invalid(
                        "complete Dalvik identity required: Lpkg/Class;->name(PARAMETERS)RETURN",
                    )
                })?;
                if !class.starts_with('L') || !class.ends_with(';') {
                    return Err(invalid(
                        "signature requires a Dalvik declaring-class descriptor",
                    ));
                }
                let start = member
                    .find('(')
                    .ok_or_else(|| invalid("complete parameter and return descriptors required"))?;
                let (name, proto) = member.split_at(start);
                if name.is_empty() || name.chars().any(|c| c.is_control() || ".;/[()".contains(c)) {
                    return Err(invalid("invalid method name"));
                }
                prototype(proto)?;
                (class_descriptor(class)?, Some((name, proto)), None)
            }
            Selector::MethodIndex {
                class,
                method_index,
            } => {
                limits.check_target(class)?;
                (class_descriptor(class)?, None, Some(*method_index))
            }
        };
        if let Some(entry) = &request.options.entry {
            limits.check_target(entry)?;
            if entry.is_empty() {
                return Err(invalid("DEX entry cannot be empty"));
            }
        }
        let selector = match name_proto {
            Some((name, prototype)) => hooks::Selector::Signature {
                class: &class,
                name,
                prototype,
            },
            None => hooks::Selector::MethodIndex {
                class: &class,
                index: index.expect("index selector"),
            },
        };
        let lookup = hooks::resolve(
            request.input.bytes(),
            selector,
            request.options.entry.as_deref(),
            hooks::Limits {
                expanded_bytes: limits.expanded_bytes,
                target_bytes: limits.target_bytes,
                analysis_steps: limits.analysis_steps,
                timeout: std::time::Duration::from_millis(limits.analysis_timeout_ms),
            },
        )
        .map_err(|failure| {
            let kind = match &failure {
                hooks::Failure::InvalidInput(_) => ErrorKind::InvalidInput,
                hooks::Failure::ResourceLimit(_) => ErrorKind::ResourceLimit,
                hooks::Failure::Timeout => ErrorKind::AnalysisTimeout,
            };
            Diagnostic::new(kind, Stage::Analysis, failure.to_string())
        })?;
        if lookup.definitions.len() > 1 || lookup.methods.len() > 1 {
            return Err(diagnostic(
                ErrorKind::Ambiguous,
                format!(
                    "duplicate declared definitions in {}; select an exact DEX entry (repeated physical names remain ambiguous)",
                    lookup.definitions.join(", ")
                ),
            ));
        }
        let method = lookup.methods.into_iter().next().ok_or_else(|| diagnostic(ErrorKind::NotFound, "method is not declared by the selected class/entry; method-index is a method_ids index, not a class-local slot"))?;
        if method
            .same_prototype_names
            .iter()
            .any(|name| bridge_name(name) == bridge_name(&method.name))
        {
            return Err(diagnostic(
                ErrorKind::Ambiguous,
                "original method names collide in the pinned Java bridge member model",
            ));
        }
        if method.name == "<clinit>" || method.access_flags & 0x400 != 0 {
            return Err(diagnostic(
                ErrorKind::Unsupported,
                "abstract methods and class initializers have no supported replacement contract",
            ));
        }
        let (parameters, result) = prototype(&method.prototype).map_err(|_| {
            diagnostic(
                ErrorKind::InvalidInput,
                "selected DEX method has invalid type descriptors",
            )
        })?;
        if method.name == "<init>" && (result != "V" || method.access_flags & 8 != 0) {
            return Err(diagnostic(
                ErrorKind::InvalidInput,
                "constructor must be instance and return void",
            ));
        }
        render(request, &method, &parameters, result, limits)
    }

    fn bridge_name(name: &str) -> String {
        if name == "<init>" {
            "$init".into()
        } else if name.starts_with('$') {
            format!("_{name}")
        } else {
            name.into()
        }
    }
    fn overload_name(descriptor: &str) -> String {
        match descriptor {
            "Z" => "boolean".into(),
            "B" => "byte".into(),
            "C" => "char".into(),
            "S" => "short".into(),
            "I" => "int".into(),
            "J" => "long".into(),
            "F" => "float".into(),
            "D" => "double".into(),
            "V" => "void".into(),
            d if d.starts_with('L') => d[1..d.len() - 1].replace('/', "."),
            d => d.replace('/', "."),
        }
    }
    fn ts_type(descriptor: &str) -> String {
        let rank = descriptor.bytes().take_while(|&c| c == b'[').count();
        let element = &descriptor[rank..];
        let mut result = match element {
            "V" => "void",
            "Z" => "boolean",
            "C" => "string",
            "J" => "Int64",
            "B" | "S" | "I" | "F" | "D" => "number",
            "Ljava/lang/String;" => "string | null",
            _ => "__rxdWrapper | null",
        }
        .to_owned();
        for dimension in 0..rank {
            result = if dimension == 0 && !element.starts_with('L') {
                format!("__rxdPrimitiveArray<{result}> | null")
            } else {
                format!("Array<{result}> | null")
            };
        }
        result
    }
    fn render(
        request: &Request,
        method: &hooks::DeclaredMethod,
        parameters: &[&str],
        result: &str,
        limits: Limits,
    ) -> Result<Artifact> {
        let ts = request.options.language == Language::TypeScript;
        let mut source = SourceBuilder::new(limits)?;
        if request.options.import_java_bridge {
            source.push("import Java from \"frida-java-bridge\";\n")?;
        }
        if ts && !request.options.import_java_bridge {
            // Both constructs erase; identical global var declarations merge. A caller
            // import naturally shadows this global without loading another bridge.
            source.push("import type {} from \"frida-java-bridge\";\ndeclare global { var Java: typeof import(\"frida-java-bridge\").default; }\n")?;
        }
        source.push("{\n")?;
        if ts {
            source.push("type __rxdWrapper = import(\"frida-java-bridge\").default.Wrapper;\n// Primitive-array proxies are not ordinary JS Arrays (bridge 7.0.13).\ntype __rxdPrimitiveArray<T> = { readonly length: number; [index: number]: T; $dispose(): void };\n")?;
        }
        let target = format!("{}->{}{}", method.class, method.name, method.prototype);
        limits.check_target(&target)?;
        source.push(&setup_prelude(&target, request.options.language))?;
        source.push(&format!("const __rxdIdentity = {{ sourceId: {}, entry: {}, methodIndex: {}, accessFlags: {} }};\n", js_string(request.input.source_id()), js_string(&method.entry), method.method_index, method.access_flags))?;
        source.push("// Waiting does not prove Java.use began. Process crashes cannot be caught; logs may be delayed.\n")?;
        source.push(&setup_stage(Stage::DexSchedule, StageEvent::Begin))?;
        source.push(&setup_stage(Stage::DexSchedule, StageEvent::Waiting))?;
        source.push(
            "let __rxdEntered = false;\ntry {\nJava.perform(() => {\n__rxdEntered = true;\ntry {\n",
        )?;
        source.push(&setup_stage(Stage::DexJavaUse, StageEvent::Begin))?;
        source.push(&format!(
            "const __rxdClass = Java.use({});\n",
            js_string(&overload_name(&method.class))
        ))?;
        source.push(&setup_stage(Stage::DexJavaUse, StageEvent::Success))?;
        source.push(&setup_stage(Stage::DexOverload, StageEvent::Begin))?;
        let name = bridge_name(&method.name);
        let dispatcher_type = if ts {
            ": import(\"frida-java-bridge\").default.MethodDispatcher"
        } else {
            ""
        };
        if method.name == "class" {
            source.push("// Pinned bridge 7.0.13 reserves class on its wrapper proxy.\nif (typeof __rxdClass.$find !== \"function\") { throw new Error(\"Java bridge member factory unavailable\"); }\nconst __rxdFactory = __rxdClass.$find(\"class\");\nif (typeof __rxdFactory !== \"function\") { throw new Error(\"Java bridge declared method unavailable\"); }\n")?;
            source.push(&format!(
                "const __rxdDispatcher{dispatcher_type} = __rxdFactory(__rxdClass);\n"
            ))?;
        } else {
            source.push(&format!(
                "const __rxdDispatcher{dispatcher_type} = __rxdClass[{}];\n",
                js_string(&name)
            ))?;
        }
        source.push("if (__rxdDispatcher == null || !Array.isArray(__rxdDispatcher.overloads)) { throw new Error(\"Java bridge method dispatcher unavailable\"); }\n")?;
        let overloads = parameters
            .iter()
            .map(|p| js_string(&overload_name(p)))
            .collect::<Vec<_>>()
            .join(", ");
        source.push(&format!(
            "const __rxdExpectedArgs = [{overloads}]{};\n",
            if ts { " as string[]" } else { "" }
        ))?;
        source.push(&format!("const __rxdMatches = __rxdDispatcher.overloads.filter(__rxdCandidate =>\n__rxdCandidate.returnType.className === {} &&\n__rxdCandidate.argumentTypes.length === __rxdExpectedArgs.length &&\n__rxdCandidate.argumentTypes.every((__rxdType, __rxdIndex) => __rxdType.className === __rxdExpectedArgs[__rxdIndex]));\n", js_string(&overload_name(result))))?;
        source.push("if (__rxdMatches.length !== 1) { throw new Error(\"exact declared overload missing or ambiguous in Java bridge\"); }\n")?;
        let types = parameters
            .iter()
            .map(|p| ts_type(p))
            .collect::<Vec<_>>()
            .join(", ");
        source.push("const __rxdOriginal = __rxdMatches[0]")?;
        if ts {
            source.push(&format!(
                " as import(\"frida-java-bridge\").default.Method<{{}}, [{types}], {}>",
                ts_type(result)
            ))?;
        }
        source.push(";\n")?;
        source.push(&setup_stage(Stage::DexOverload, StageEvent::Success))?;
        source.push(&setup_stage(Stage::DexImplementation, StageEvent::Begin))?;
        let args = (0..parameters.len())
            .map(|n| format!("arg{n}"))
            .collect::<Vec<_>>();
        let mut formal = Vec::new();
        if ts {
            formal.push("this: __rxdWrapper".into());
        }
        for (arg, parameter) in args.iter().zip(parameters) {
            formal.push(if ts {
                format!("{arg}: {}", ts_type(parameter))
            } else {
                arg.clone()
            });
        }
        source.push(&format!("__rxdOriginal.implementation = function ({}){} {{\n__rxdLog(\"call\", __rxdIdentity, [{}]);\ntry {{\n", formal.join(", "), if ts { format!(": {}", ts_type(result)) } else { String::new() }, args.join(", ")))?;
        let receiver = if method.access_flags & 8 != 0 {
            "__rxdClass"
        } else {
            "this"
        };
        let call_args = std::iter::once(receiver.to_owned())
            .chain(args)
            .collect::<Vec<_>>()
            .join(", ");
        if result == "V" {
            source.push(&format!(
                "__rxdOriginal.call({call_args});\n__rxdLog(\"return\");\nreturn;\n"
            ))?;
        } else {
            source.push(&format!("const __rxdResult{} = __rxdOriginal.call({call_args});\n__rxdLog(\"return\", __rxdResult);\nreturn __rxdResult;\n", if ts { format!(": {}", ts_type(result)) } else { String::new() }))?;
        }
        source.push("} catch (__rxdThrown) {\n__rxdLog(\"throw\", __rxdThrown);\nthrow __rxdThrown;\n}\n};\n")?;
        source.push(&setup_stage(Stage::DexImplementation, StageEvent::Success))?;
        source.push("}\n")?;
        source.push(setup_catch())?;
        source.push("});\n} catch (__rxdError) {\nif (!__rxdEntered) { __rxdLog(\"failure\", __rxdError instanceof Error ? (__rxdError.stack ?? __rxdError.message) : __rxdError); }\nthrow __rxdError;\n}\n}\n")?;
        Ok(Artifact::Source(SourceArtifact::new(
            request.options.language,
            source.finish(),
            limits,
        )?))
    }
}
