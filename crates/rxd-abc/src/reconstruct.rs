use crate::{
    Abc, Coordinate, Decoded, EntityKind, Instruction, LiteralValue, Method, Module, Name, Operand,
    reader::{invalid, resource},
    structure::{Graph, unsupported},
};
use rxd_core::{Error, Result, output::Output};
use std::{collections::HashSet, io::Write, sync::Arc};

struct SourceUnit<'a> {
    method: Method,
    decoded: Decoded<'a>,
}

#[derive(Clone, Copy)]
struct Loop {
    header: usize,
    exit: usize,
}

#[derive(Clone, Copy)]
struct TryRegion {
    start: usize,
    end: usize,
    handler: usize,
    join: usize,
    bypass: Option<usize>,
    start_pc: u32,
    end_pc: u32,
    handler_pc: u32,
    join_pc: u32,
}

struct Writer<'a, 'b> {
    abc: &'a Abc<'b>,
    decoded: &'a Decoded<'b>,
    graph: Graph,
    visited: Vec<bool>,
    ordinary_receivers: Vec<bool>,
    arrow_plan: Option<crate::arrow_plan::ArrowPlan>,
    async_plan: Option<crate::async_plan::AsyncPlan>,
    try_regions: Vec<TryRegion>,
    in_try: bool,
    uses_context: bool,
    module: Option<Arc<Module>>,
    out: Output,
}

fn identifier(name: &Name) -> Option<&str> {
    let text = name.scalar_text()?;
    let mut chars = text.bytes();
    let start = chars.next()?;
    if !(start.is_ascii_alphabetic() || matches!(start, b'_' | b'$'))
        || !chars.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
        || matches!(
            text,
            "acc"
                | "arguments"
                | "eval"
                | "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "debugger"
                | "default"
                | "delete"
                | "do"
                | "else"
                | "enum"
                | "export"
                | "extends"
                | "false"
                | "finally"
                | "for"
                | "function"
                | "if"
                | "import"
                | "in"
                | "instanceof"
                | "new"
                | "null"
                | "return"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "true"
                | "try"
                | "typeof"
                | "var"
                | "void"
                | "while"
                | "with"
                | "yield"
                | "await"
                | "implements"
                | "interface"
                | "let"
                | "package"
                | "private"
                | "protected"
                | "public"
                | "static"
        )
        || text.starts_with("_abc_")
        || text.starts_with("__abc_")
        || text
            .strip_prefix(['v', 'p'])
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    Some(text)
}

fn missing(instruction: &Instruction) -> Error {
    unsupported(format!(
        "source reconstruction does not support {} at PC {:#x}",
        instruction.opcode.name, instruction.pc
    ))
}

impl<'a, 'b> Writer<'a, 'b> {
    fn indent(&mut self, depth: usize) -> Result<()> {
        if depth > self.abc.budget.ast_depth {
            return Err(resource("ABC source nesting exceeds depth guard"));
        }
        for _ in 0..depth {
            self.out.write_all(b"    ")?;
        }
        Ok(())
    }

    fn line(&mut self, depth: usize, text: &str) -> Result<()> {
        self.indent(depth)?;
        writeln!(self.out, "{text}")?;
        Ok(())
    }

    fn literal_string(&mut self, instruction: &Instruction, index: usize) -> Result<()> {
        let offset = instruction.entity(index, EntityKind::String)?;
        self.abc.name(offset)?.write_quoted(&mut self.out)?;
        Ok(())
    }

    fn number(&mut self, value: f64) -> Result<()> {
        if value.is_nan() {
            self.out.write_all(b"(0 / 0)")?;
        } else if value == f64::INFINITY {
            self.out.write_all(b"(1 / 0)")?;
        } else if value == f64::NEG_INFINITY {
            self.out.write_all(b"(-1 / 0)")?;
        } else {
            write!(self.out, "{value:?}")?;
        }
        Ok(())
    }

    fn literal(&mut self, value: &LiteralValue) -> Result<()> {
        match value {
            LiteralValue::Integer(bits) => write!(self.out, "{}", *bits as i32)?,
            LiteralValue::Float(bits) => self.number(f64::from(f32::from_bits(*bits)))?,
            LiteralValue::Double(bits) => self.number(f64::from_bits(*bits))?,
            LiteralValue::String(text) => text.write_quoted(&mut self.out)?,
            LiteralValue::Byte(0) => self.out.write_all(b"false")?,
            LiteralValue::Byte(1) => self.out.write_all(b"true")?,
            _ => {
                return Err(unsupported(
                    "literal array value needs method, accessor or nested-literal reconstruction",
                ));
            }
        }
        Ok(())
    }

    fn buffer(&mut self, instruction: &Instruction, object: bool) -> Result<()> {
        let offset = instruction.entity(1, EntityKind::Literal)?;
        let literals = self.abc.literals(offset)?;
        if object && !literals.len().is_multiple_of(2) {
            return Err(unsupported("object literal contains non-pair entries"));
        }
        self.out.write_all(if object { b"{" } else { b"[" })?;
        let mut index = 0;
        while index < literals.len() {
            if index > 0 {
                self.out.write_all(b", ")?;
            }
            if object {
                let LiteralValue::String(key) = &literals[index].value else {
                    return Err(unsupported("object literal has a non-string key"));
                };
                self.out.write_all(b"[")?;
                key.write_quoted(&mut self.out)?;
                self.out.write_all(b"]: ")?;
                index += 1;
            }
            match (&literals[index].tag, &literals[index].value) {
                (1, LiteralValue::Byte(_)) | (2..=5, _) => self.literal(&literals[index].value)?,
                (0xff, LiteralValue::Byte(_)) => self.out.write_all(b"null")?,
                _ => {
                    return Err(unsupported(
                        "literal array value needs method, accessor or nested-literal reconstruction",
                    ));
                }
            }
            index += 1;
        }
        self.out.write_all(if object { b"}" } else { b"]" })?;
        Ok(())
    }

    fn module(&self) -> Result<&Module> {
        self.module
            .as_deref()
            .ok_or_else(|| invalid("ABC method references missing module metadata"))
    }

    fn import(&self, index: u32) -> Result<&crate::Import> {
        let module = self.module()?;
        let index = index as usize;
        module
            .regular_imports
            .get(index)
            .or_else(|| {
                index
                    .checked_sub(module.regular_imports.len())
                    .and_then(|index| module.namespace_imports.get(index))
            })
            .ok_or_else(|| invalid("ABC import slot is outside module metadata"))
    }

    fn local(&self, index: u32) -> Result<Arc<Name>> {
        let entry = self
            .module()?
            .local_exports
            .get(index as usize)
            .ok_or_else(|| invalid("ABC local module slot is outside module metadata"))?;
        Ok(Arc::clone(&entry.local_name))
    }

    fn imports<'c>(
        &mut self,
        instructions: impl IntoIterator<Item = &'c Instruction>,
    ) -> Result<()> {
        let mut imported = Vec::new();
        let mut namespaces = Vec::new();
        for instruction in instructions {
            match instruction.opcode.name {
                "ldexternalmodulevar" | "wide.ldexternalmodulevar" => {
                    imported
                        .try_reserve(1)
                        .map_err(|_| resource("ABC import selection allocation failed"))?;
                    imported.push(instruction.unsigned(0)?);
                }
                "getmodulenamespace" | "wide.getmodulenamespace" => {
                    namespaces
                        .try_reserve(1)
                        .map_err(|_| resource("ABC namespace selection allocation failed"))?;
                    namespaces.push(instruction.unsigned(0)?);
                }
                _ => {}
            }
        }
        imported.sort_unstable();
        imported.dedup();
        namespaces.sort_unstable();
        namespaces.dedup();
        for index in imported {
            let entry = self.import(index)?;
            let request = Arc::clone(
                self.module()?
                    .requests
                    .get(entry.request as usize)
                    .ok_or_else(|| invalid("ABC import module request is out of range"))?,
            );
            let export = entry.import_name.clone();
            match export {
                None => write!(self.out, "import * as __abc_import_{index} from ")?,
                Some(name) if name.text == "default" => {
                    write!(self.out, "import __abc_import_{index} from ")?
                }
                Some(name) => {
                    self.out.write_all(b"import { ")?;
                    name.write_quoted(&mut self.out)?;
                    write!(self.out, " as __abc_import_{index} }} from ")?;
                }
            }
            request.write_quoted(&mut self.out)?;
            self.out.write_all(b";\n")?;
        }
        for index in namespaces {
            let request = Arc::clone(
                self.module()?
                    .requests
                    .get(index as usize)
                    .ok_or_else(|| invalid("ABC module namespace request is out of range"))?,
            );
            write!(self.out, "import * as __abc_namespace_{index} from ")?;
            request.write_quoted(&mut self.out)?;
            self.out.write_all(b";\n")?;
        }
        Ok(())
    }

    fn module_guard(&self, instruction: &Instruction) -> bool {
        if instruction.opcode.name != "throw.undefinedifholewithname" {
            return false;
        }
        if self
            .graph
            .blocks
            .binary_search_by_key(&instruction.pc, |block| {
                self.decoded.instructions[block.first].pc
            })
            .is_ok()
        {
            return false;
        }
        let Ok(index) = self
            .decoded
            .instructions
            .binary_search_by_key(&instruction.pc, |item| item.pc)
        else {
            return false;
        };
        index.checked_sub(1).is_some_and(|previous| {
            let previous = &self.decoded.instructions[previous];
            previous.end() == instruction.pc
                && matches!(
                    previous.opcode.name,
                    "ldexternalmodulevar"
                        | "wide.ldexternalmodulevar"
                        | "ldlocalmodulevar"
                        | "wide.ldlocalmodulevar"
                )
        })
    }

    fn ordinary_receiver(&self, instruction: &Instruction) -> Result<()> {
        let index = self
            .decoded
            .instructions
            .binary_search_by_key(&instruction.pc, |item| item.pc)
            .map_err(|_| invalid("ABC instruction is outside decoded code"))?;
        if !self.ordinary_receivers[index] {
            return Err(unsupported(format!(
                "own-property target at PC {:#x} is not proven to be an ordinary literal",
                instruction.pc
            )));
        }
        Ok(())
    }

    fn args(&mut self, registers: impl Iterator<Item = u32>) -> Result<()> {
        for (index, register) in registers.enumerate() {
            if index > 0 {
                self.out.write_all(b", ")?;
            }
            write!(self.out, "v{register}")?;
        }
        Ok(())
    }

    fn emit_instruction(&mut self, instruction: &Instruction, depth: usize) -> Result<()> {
        let name = instruction.opcode.name;
        if self
            .arrow_plan
            .as_ref()
            .is_some_and(|plan| plan.skips(instruction.pc))
        {
            return Ok(());
        }
        if let Some(plan) = &self.async_plan {
            if instruction.pc == plan.enter
                || instruction.pc == plan.save
                || instruction.pc == plan.reject_return
                || name == "asyncfunctionresolve"
            {
                return Ok(());
            }
            if name == "asyncfunctionreject" {
                return self.line(depth, "throw acc;");
            }
        }
        if self.module_guard(instruction) {
            return Ok(());
        }
        match name {
            "nop" => return Ok(()),
            "return" => return self.line(depth, "return acc;"),
            "returnundefined" => return self.line(depth, "return;"),
            "throw" => return self.line(depth, "throw acc;"),
            _ => {}
        }
        self.indent(depth)?;
        match name {
            "lda" => write!(self.out, "acc = v{}", instruction.register(0)?)?,
            "sta" => write!(self.out, "v{} = acc", instruction.register(0)?)?,
            "mov" => write!(
                self.out,
                "v{} = v{}",
                instruction.register(0)?,
                instruction.register(1)?
            )?,
            "ldai" => match instruction.operands.first() {
                Some(Operand::Signed(value)) => write!(self.out, "acc = {value}")?,
                _ => return Err(invalid("invalid ldai immediate")),
            },
            "fldai" => match instruction.operands.first() {
                Some(Operand::Float(bits)) => {
                    let value = f64::from_bits(*bits);
                    self.out.write_all(b"acc = ")?;
                    self.number(value)?;
                }
                _ => return Err(invalid("invalid fldai immediate")),
            },
            "lda.str" => {
                self.out.write_all(b"acc = ")?;
                self.literal_string(instruction, 0)?;
            }
            "ldtrue" => self.out.write_all(b"acc = true")?,
            "ldfalse" => self.out.write_all(b"acc = false")?,
            "ldnull" => self.out.write_all(b"acc = null")?,
            "ldundefined" => self.out.write_all(b"acc = void 0")?,
            "ldhole" => self.out.write_all(b"acc = __abc_context.hole")?,
            "newlexenv" | "wide.newlexenv" | "newlexenvwithname" | "wide.newlexenvwithname" => {
                writeln!(
                    self.out,
                    "__abc_env = {{parent: __abc_env, slots: Array({}).fill(__abc_context.hole)}};",
                    instruction.unsigned(0)?
                )?;
                self.indent(depth)?;
                self.out.write_all(b"acc = __abc_env")?;
            }
            "poplexenv" => self.out.write_all(b"__abc_env = __abc_env.parent")?,
            "ldlexvar" | "wide.ldlexvar" => {
                self.out.write_all(b"acc = __abc_env")?;
                for _ in 0..instruction.unsigned(0)? {
                    self.out.write_all(b".parent")?;
                }
                write!(self.out, ".slots[{}]", instruction.unsigned(1)?)?;
            }
            "stlexvar" | "wide.stlexvar" => {
                self.out.write_all(b"__abc_env")?;
                for _ in 0..instruction.unsigned(0)? {
                    self.out.write_all(b".parent")?;
                }
                write!(self.out, ".slots[{}] = acc", instruction.unsigned(1)?)?;
            }
            "throw.undefinedifholewithname" => {
                if !self.uses_context {
                    return Err(missing(instruction));
                }
                self.out
                    .write_all(b"if (acc === __abc_context.hole) { throw new ReferenceError(")?;
                self.literal_string(instruction, 0)?;
                self.out.write_all(b"); }")?;
            }
            "ldnan" => self.out.write_all(b"acc = (0 / 0)")?,
            "ldinfinity" => self.out.write_all(b"acc = (1 / 0)")?,
            "ldglobal" => self.out.write_all(b"acc = globalThis")?,
            "ldthis" => self.out.write_all(b"acc = this")?,
            "ldnewtarget" if self.arrow_plan.is_some() => self.out.write_all(b"acc = void 0")?,
            "ldnewtarget" => self.out.write_all(b"acc = new.target")?,
            "ldfunction" => write!(self.out, "acc = _abc_{:x}", self.decoded.method)?,
            "definefunc" => {
                let target = instruction.entity(1, EntityKind::Method)?;
                writeln!(
                    self.out,
                    "acc = _abc_capture_{target:x}({{env: __abc_env, hole: __abc_context.hole}});"
                )?;
                self.indent(depth)?;
                write!(
                    self.out,
                    "Object.defineProperty(acc, \"length\", {{value: {}}})",
                    instruction.unsigned(2)?
                )?;
            }
            "createemptyobject" => self.out.write_all(b"acc = {}")?,
            "createemptyarray" => self.out.write_all(b"acc = []")?,
            "createarraywithbuffer" | "createobjectwithbuffer" => {
                self.out.write_all(b"acc = ")?;
                self.buffer(instruction, name == "createobjectwithbuffer")?;
            }
            "ldexternalmodulevar" | "wide.ldexternalmodulevar" => {
                let index = instruction.unsigned(0)?;
                self.import(index)?;
                write!(self.out, "acc = __abc_import_{index}")?;
            }
            "ldlocalmodulevar" | "wide.ldlocalmodulevar" | "stmodulevar" | "wide.stmodulevar" => {
                let local = self.local(instruction.unsigned(0)?)?;
                let text = identifier(&local).ok_or_else(|| {
                    unsupported(
                        "local module binding cannot be expressed as an unshadowed JS identifier",
                    )
                })?;
                if name.contains("stmodulevar") {
                    write!(self.out, "{text} = acc")?;
                } else {
                    write!(self.out, "acc = {text}")?;
                }
            }
            "getmodulenamespace" | "wide.getmodulenamespace" => {
                let index = instruction.unsigned(0)?;
                if self.module()?.requests.get(index as usize).is_none() {
                    return Err(invalid("ABC namespace index is outside module metadata"));
                }
                write!(self.out, "acc = __abc_namespace_{index}")?;
            }
            "tryldglobalbyname" => {
                let offset = instruction.entity(1, EntityKind::String)?;
                let name = self.abc.name(offset)?;
                let text = identifier(&name).ok_or_else(|| {
                    unsupported("global binding cannot be expressed as an unshadowed JS identifier")
                })?;
                write!(self.out, "acc = {text}")?;
            }
            "istrue" | "callruntime.istrue" => self.out.write_all(b"acc = !!acc")?,
            "isfalse" | "callruntime.isfalse" => self.out.write_all(b"acc = !acc")?,
            "typeof" => self.out.write_all(b"acc = typeof acc")?,
            "neg" => self.out.write_all(b"acc = -acc")?,
            "not" => self.out.write_all(b"acc = ~acc")?,
            "tonumber" => self.out.write_all(b"acc = +acc")?,
            "inc" => self.out.write_all(b"++acc")?,
            "dec" => self.out.write_all(b"--acc")?,
            "add2" | "sub2" | "mul2" | "div2" | "mod2" | "exp" | "eq" | "noteq" | "less"
            | "lesseq" | "greater" | "greatereq" | "stricteq" | "strictnoteq" | "shl2" | "shr2"
            | "ashr2" | "and2" | "or2" | "xor2" | "instanceof" | "isin" => {
                let operator = match name {
                    "add2" => "+",
                    "sub2" => "-",
                    "mul2" => "*",
                    "div2" => "/",
                    "mod2" => "%",
                    "exp" => "**",
                    "eq" => "==",
                    "noteq" => "!=",
                    "less" => "<",
                    "lesseq" => "<=",
                    "greater" => ">",
                    "greatereq" => ">=",
                    "stricteq" => "===",
                    "strictnoteq" => "!==",
                    "shl2" => "<<",
                    "shr2" => ">>>",
                    "ashr2" => ">>",
                    "and2" => "&",
                    "or2" => "|",
                    "xor2" => "^",
                    "instanceof" => "instanceof",
                    "isin" => "in",
                    _ => unreachable!(),
                };
                write!(
                    self.out,
                    "acc = v{} {operator} acc",
                    instruction.register(1)?
                )?;
            }
            "ldobjbyname" => {
                self.out.write_all(b"acc = acc[")?;
                self.literal_string(instruction, 1)?;
                self.out.write_all(b"]")?;
            }
            "stobjbyname" => {
                write!(self.out, "v{}[", instruction.register(2)?)?;
                self.literal_string(instruction, 1)?;
                self.out.write_all(b"] = acc")?;
            }
            "ldobjbyvalue" => write!(self.out, "acc = v{}[acc]", instruction.register(1)?)?,
            "stobjbyvalue" => write!(
                self.out,
                "v{}[v{}] = acc",
                instruction.register(1)?,
                instruction.register(2)?
            )?,
            "ldobjbyindex" | "wide.ldobjbyindex" => {
                write!(
                    self.out,
                    "acc = acc[{}]",
                    instruction.unsigned(usize::from(name == "ldobjbyindex"))?
                )?;
            }
            "stobjbyindex" | "wide.stobjbyindex" => {
                let wide = name.starts_with("wide.");
                write!(
                    self.out,
                    "v{}[{}] = acc",
                    instruction.register(usize::from(!wide))?,
                    instruction.unsigned(if wide { 1 } else { 2 })?
                )?;
            }
            "definefieldbyname" | "definepropertybyname" => {
                write!(
                    self.out,
                    "Object.defineProperty(v{}, ",
                    instruction.register(2)?
                )?;
                self.literal_string(instruction, 1)?;
                self.out.write_all(
                    b", {value: acc, writable: true, enumerable: true, configurable: true})",
                )?;
            }
            "stownbyname" | "stownbyvalue" | "stownbyindex" | "wide.stownbyindex" => {
                self.ordinary_receiver(instruction)?;
                let object = if name == "stownbyname" {
                    instruction.register(2)?
                } else if name == "wide.stownbyindex" {
                    instruction.register(0)?
                } else {
                    instruction.register(1)?
                };
                write!(self.out, "Object.defineProperty(v{object}, ")?;
                match name {
                    "stownbyname" => self.literal_string(instruction, 1)?,
                    "stownbyvalue" => write!(self.out, "v{}", instruction.register(2)?)?,
                    "stownbyindex" => write!(self.out, "{}", instruction.unsigned(2)?)?,
                    _ => write!(self.out, "{}", instruction.unsigned(1)?)?,
                }
                self.out.write_all(
                    b", {value: acc, writable: true, enumerable: true, configurable: true})",
                )?;
            }
            "callruntime.definefieldbyvalue" => {
                write!(
                    self.out,
                    "Object.defineProperty(v{}, v{}, {{value: acc, writable: true, enumerable: true, configurable: true}})",
                    instruction.register(2)?,
                    instruction.register(1)?
                )?;
            }
            "callruntime.definefieldbyindex" => {
                write!(
                    self.out,
                    "Object.defineProperty(v{}, {}, {{value: acc, writable: true, enumerable: true, configurable: true}})",
                    instruction.register(2)?,
                    instruction.unsigned(1)?
                )?;
            }
            "copyrestargs" | "wide.copyrestargs" => {
                write!(
                    self.out,
                    "acc = Array.prototype.slice.call(arguments, {})",
                    instruction.unsigned(0)?
                )?;
            }
            "callarg0" | "callarg1" | "callargs2" | "callargs3" => {
                self.out.write_all(b"acc = acc(")?;
                let mut registers = Vec::new();
                registers
                    .try_reserve_exact(instruction.operands.len())
                    .map_err(|_| resource("ABC call allocation failed"))?;
                for index in 1..instruction.operands.len() {
                    registers.push(instruction.register(index)?);
                }
                self.args(registers.into_iter())?;
                self.out.write_all(b")")?;
            }
            "callthis0" | "callthis1" | "callthis2" | "callthis3" => {
                write!(
                    self.out,
                    "acc = Reflect.apply(acc, v{}, [",
                    instruction.register(1)?
                )?;
                let mut registers = Vec::new();
                registers
                    .try_reserve_exact(instruction.operands.len())
                    .map_err(|_| resource("ABC call allocation failed"))?;
                for index in 2..instruction.operands.len() {
                    registers.push(instruction.register(index)?);
                }
                self.args(registers.into_iter())?;
                self.out.write_all(b"])")?;
            }
            "callrange" | "wide.callrange" | "callthisrange" | "wide.callthisrange" => {
                let wide = name.starts_with("wide.");
                let count = instruction.unsigned(if wide { 0 } else { 1 })?;
                let base = instruction.register(if wide { 1 } else { 2 })?;
                if name.contains("callthisrange") {
                    write!(self.out, "acc = Reflect.apply(acc, v{base}, [")?;
                    self.args((base + 1)..(base + 1 + count))?;
                    self.out.write_all(b"])")?;
                } else {
                    self.out.write_all(b"acc = acc(")?;
                    self.args(base..base + count)?;
                    self.out.write_all(b")")?;
                }
            }
            "newobjrange" | "wide.newobjrange" => {
                let wide = name.starts_with("wide.");
                let count = instruction.unsigned(if wide { 0 } else { 1 })?;
                let base = instruction.register(if wide { 1 } else { 2 })?;
                if count == 0 {
                    return Err(invalid("ABC constructor range has zero entries"));
                }
                write!(self.out, "acc = new v{base}(")?;
                self.args((base + 1)..(base + count))?;
                self.out.write_all(b")")?;
            }
            "dynamicimport" => self.out.write_all(b"acc = import(acc)")?,
            _ => return Err(missing(instruction)),
        }
        self.out.write_all(b";\n")?;
        Ok(())
    }

    fn condition(&mut self, instruction: &Instruction) -> Result<()> {
        match instruction.opcode.name {
            // The VM branches only for tagged false/true or numeric zero/nonzero.
            "jeqz" => self.out.write_all(b"(acc === false || acc === 0)")?,
            "jnez" => self
                .out
                .write_all(b"(acc === true || (typeof acc === \"number\" && acc !== 0))")?,
            _ => return Err(missing(instruction)),
        }
        Ok(())
    }

    fn jump_target(&self, instruction: &Instruction) -> Result<usize> {
        self.graph.target(
            &self.decoded.instructions,
            instruction
                .branch
                .ok_or_else(|| invalid("missing ABC branch target"))?,
        )
    }

    fn forward_labels(&mut self, base_depth: usize) -> Result<()> {
        if !self.try_regions.is_empty() {
            return Err(unsupported(
                "labelled control flow cannot cross exception ranges",
            ));
        }
        let mut targets = Vec::new();
        targets
            .try_reserve_exact(self.graph.blocks.len())
            .map_err(|_| resource("ABC label allocation failed"))?;
        for (index, block) in self.graph.blocks.iter().enumerate() {
            if !self.graph.reachable[index] {
                continue;
            }
            let instruction = &self.decoded.instructions[block.end - 1];
            if instruction.branch.is_some() {
                let target = self.jump_target(instruction)?;
                if target <= index {
                    return Err(unsupported("backward branch requires loop structuring"));
                }
                if target != index + 1 {
                    targets.push(target);
                }
            }
        }
        targets.sort_unstable();
        targets.dedup();
        let mut depth = base_depth;
        for target in targets.iter().rev() {
            self.indent(depth)?;
            writeln!(
                self.out,
                "__abc_to_{:x}: {{",
                self.decoded.instructions[self.graph.blocks[*target].first].pc
            )?;
            depth += 1;
        }
        let mut next = 0;
        for index in 0..self.graph.blocks.len() {
            if targets.get(next) == Some(&index) {
                depth -= 1;
                self.line(depth, "}")?;
                next += 1;
            }
            if !self.graph.reachable[index] {
                continue;
            }
            self.visited[index] = true;
            let block = self.graph.blocks[index];
            let last = &self.decoded.instructions[block.end - 1];
            let terminal =
                last.opcode.is_jump() || last.opcode.is_return() || last.opcode.name == "throw";
            for instruction in
                &self.decoded.instructions[block.first..block.end - usize::from(terminal)]
            {
                self.emit_instruction(instruction, depth)?;
            }
            if last.opcode.is_jump() {
                let target = self.jump_target(last)?;
                if target == index + 1 {
                    continue;
                }
                let target_pc = self.decoded.instructions[self.graph.blocks[target].first].pc;
                self.indent(depth)?;
                if last.opcode.is_conditional() {
                    self.out.write_all(b"if (")?;
                    self.condition(last)?;
                    writeln!(self.out, ") {{ break __abc_to_{target_pc:x}; }}")?;
                } else {
                    writeln!(self.out, "break __abc_to_{target_pc:x};")?;
                }
            } else if terminal {
                self.emit_instruction(last, depth)?;
            }
        }
        if next != targets.len() {
            return Err(invalid("ABC control-flow target was not emitted"));
        }
        Ok(())
    }

    fn range(
        &mut self,
        start: usize,
        end: usize,
        leave: usize,
        context: Option<Loop>,
        depth: usize,
    ) -> Result<()> {
        if depth > self.abc.budget.ast_depth {
            return Err(resource("ABC structure depth exceeds recursion guard"));
        }
        let mut cursor = start;
        while cursor < end {
            if let Ok(index) = self
                .try_regions
                .binary_search_by_key(&cursor, |region| region.start)
                && !self.in_try
            {
                let region = self.try_regions[index];
                if region.join > end {
                    return Err(unsupported("try/catch crosses a structured region"));
                }
                self.line(depth, "try {")?;
                self.in_try = true;
                self.range(region.start, region.end, region.end, context, depth + 1)?;
                self.in_try = false;
                if let Some(bypass) = region.bypass {
                    if self.visited[bypass] {
                        return Err(unsupported("exception bypass is shared"));
                    }
                    self.visited[bypass] = true;
                }
                self.line(depth, "} catch (__abc_exception) {")?;
                self.line(depth + 1, "acc = __abc_exception;")?;
                self.range(region.handler, region.join, region.join, context, depth + 1)?;
                self.line(depth, "}")?;
                cursor = region.join;
                continue;
            }
            if !self.graph.reachable[cursor] {
                return Err(unsupported(
                    "unreachable basic block inside a source region",
                ));
            }
            if self.visited[cursor] {
                return Err(unsupported("overlapping ABC source regions"));
            }
            self.visited[cursor] = true;
            let block = self.graph.blocks[cursor];
            let last = &self.decoded.instructions[block.end - 1];
            if last.opcode.is_conditional() {
                let target = self.jump_target(last)?;
                if target <= end && target > cursor + 1 {
                    let previous = self.graph.blocks[target - 1];
                    let tail = &self.decoded.instructions[previous.end - 1];
                    if tail.opcode.is_jump()
                        && !tail.opcode.is_conditional()
                        && self.jump_target(tail)? == cursor
                    {
                        self.line(depth, "while (true) {")?;
                        for instruction in &self.decoded.instructions[block.first..block.end - 1] {
                            self.emit_instruction(instruction, depth + 1)?;
                        }
                        self.indent(depth + 1)?;
                        self.out.write_all(b"if (")?;
                        self.condition(last)?;
                        self.out.write_all(b") { break; }\n")?;
                        self.range(
                            cursor + 1,
                            target,
                            cursor,
                            Some(Loop {
                                header: cursor,
                                exit: target,
                            }),
                            depth + 1,
                        )?;
                        self.line(depth, "}")?;
                        cursor = target;
                        continue;
                    }
                }
            }
            let terminator =
                last.opcode.is_jump() || last.opcode.is_return() || last.opcode.name == "throw";
            let body_end = if terminator { block.end - 1 } else { block.end };
            for instruction in &self.decoded.instructions[block.first..body_end] {
                self.emit_instruction(instruction, depth)?;
            }
            if last.opcode.is_return() || last.opcode.name == "throw" {
                self.emit_instruction(last, depth)?;
                if cursor + 1 < end {
                    return Err(unsupported(
                        "a terminal block precedes an unstructured region",
                    ));
                }
                return Ok(());
            }
            if !last.opcode.is_jump() {
                cursor += 1;
                if cursor == end && end != leave {
                    return Err(unsupported("source region falls into a different branch"));
                }
                continue;
            }
            let target = self.jump_target(last)?;
            if !last.opcode.is_conditional() {
                if cursor + 1 != end && target != cursor + 1 {
                    return Err(unsupported("unstructured ABC jump within a region"));
                }
                if target == leave && cursor + 1 == end {
                    return Ok(());
                }
                if let Some(active) = context {
                    if target == active.header {
                        self.line(depth, "continue;")?;
                        return Ok(());
                    }
                    if target == active.exit {
                        self.line(depth, "break;")?;
                        return Ok(());
                    }
                }
                if target == cursor + 1 {
                    cursor += 1;
                    continue;
                }
                return Err(unsupported("unstructured ABC jump target"));
            }
            if target == cursor + 1 {
                cursor += 1;
                continue;
            }
            if target <= cursor {
                return Err(unsupported(
                    "backward conditional branch needs loop analysis",
                ));
            }
            if let Some(active) = context
                && target == active.exit
            {
                self.indent(depth)?;
                self.out.write_all(b"if (")?;
                self.condition(last)?;
                self.out.write_all(b") { break; }\n")?;
                cursor += 1;
                continue;
            }
            if target > end || target == end && leave != end {
                return Err(unsupported(
                    "conditional branch exits its structured region",
                ));
            }
            let previous = self.graph.blocks[target - 1];
            let tail = &self.decoded.instructions[previous.end - 1];
            let join = if tail.opcode.is_jump() && !tail.opcode.is_conditional() {
                let candidate = self.jump_target(tail)?;
                (candidate > target && candidate <= end).then_some(candidate)
            } else {
                None
            };
            self.indent(depth)?;
            self.out.write_all(b"if (!")?;
            self.condition(last)?;
            self.out.write_all(b") {\n")?;
            self.range(
                cursor + 1,
                target,
                join.unwrap_or(target),
                context,
                depth + 1,
            )?;
            if let Some(join) = join {
                self.line(depth, "} else {")?;
                self.range(target, join, join, context, depth + 1)?;
                self.line(depth, "}")?;
                cursor = join;
            } else {
                self.line(depth, "}")?;
                cursor = target;
            }
        }
        Ok(())
    }
}

fn simple_tries(decoded: &Decoded<'_>, graph: &Graph) -> Result<Vec<TryRegion>> {
    let mut regions = Vec::new();
    regions
        .try_reserve_exact(decoded.code.tries.len())
        .map_err(|_| resource("ABC exception region allocation failed"))?;
    let block = |pc: u32| -> Result<usize> {
        if pc as usize == decoded.code.bytes.len() {
            Ok(graph.blocks.len())
        } else {
            graph.target(&decoded.instructions, pc)
        }
    };
    for region in &decoded.code.tries {
        let [handler] = region.catches.as_slice() else {
            return Err(unsupported(
                "typed or multiple exception handlers need additional analysis",
            ));
        };
        if handler.type_index != 0 || region.length == 0 || handler.length == 0 {
            return Err(unsupported(
                "typed or unsized exception handler is not yet supported",
            ));
        }
        let start = block(region.pc)?;
        let end = block(region.pc + region.length)?;
        let handler_start = block(handler.pc)?;
        let join = block(handler.pc + handler.length)?;
        if !(start < end && end <= handler_start && handler_start < join) {
            return Err(unsupported(
                "exception handler does not follow its protected range",
            ));
        }
        let preceding = &decoded.instructions[graph.blocks[handler_start - 1].end - 1];
        if !preceding.opcode.is_return()
            && preceding.opcode.name != "throw"
            && (!preceding.opcode.is_jump() || preceding.opcode.is_conditional())
        {
            return Err(unsupported("normal flow falls into an exception handler"));
        }
        let bypass = if end == handler_start {
            None
        } else {
            if end + 1 != handler_start {
                return Err(unsupported("exception bypass contains executable code"));
            }
            let bypass = graph.blocks[end];
            let jump = &decoded.instructions[bypass.first];
            if bypass.first + 1 != bypass.end
                || !jump.opcode.is_jump()
                || jump.opcode.is_conditional()
                || jump.branch != Some(handler.pc + handler.length)
            {
                return Err(unsupported(
                    "exception bypass is not a direct jump to the handler end",
                ));
            }
            Some(end)
        };
        regions.push(TryRegion {
            start,
            end,
            handler: handler_start,
            join,
            bypass,
            start_pc: region.pc,
            end_pc: region.pc + region.length,
            handler_pc: handler.pc,
            join_pc: handler.pc + handler.length,
        });
    }
    regions.sort_unstable_by_key(|region| region.start);
    if regions.windows(2).any(|pair| pair[0].join > pair[1].start) {
        return Err(unsupported(
            "overlapping exception handlers require nested region analysis",
        ));
    }
    for instruction in &decoded.instructions {
        let Some(target) = instruction.branch else {
            continue;
        };
        let upper = regions.partition_point(|region| region.start_pc <= target);
        let Some(region) = upper.checked_sub(1).and_then(|index| regions.get(index)) else {
            continue;
        };
        if target >= region.handler_pc
            && target < region.join_pc
            && !(region.handler_pc..region.join_pc).contains(&instruction.pc)
        {
            return Err(unsupported(
                "normal control flow enters an exception handler",
            ));
        }
        if target > region.start_pc
            && target < region.end_pc
            && !(region.start_pc..region.end_pc).contains(&instruction.pc)
        {
            return Err(unsupported(
                "control flow enters the middle of a protected range",
            ));
        }
    }
    Ok(regions)
}

impl<'a> Abc<'a> {
    pub fn decompile_target(&self, target: Coordinate) -> Result<Vec<u8>> {
        let Coordinate::Method(offset) = target else {
            return Err(unsupported(
                "record-level source reconstruction is not yet available",
            ));
        };
        let method = self.method(offset)?;
        self.decompile_method(&method)
    }

    pub fn decompile_method(&self, method: &Method) -> Result<Vec<u8>> {
        let mut units = Vec::new();
        let mut pending = Vec::new();
        let mut seen = HashSet::new();
        pending
            .try_reserve(1)
            .map_err(|_| resource("ABC definition queue allocation failed"))?;
        seen.try_reserve(1)
            .map_err(|_| resource("ABC definition set allocation failed"))?;
        pending.push(method.clone());
        seen.insert(method.offset);
        while let Some(current) = pending.pop() {
            let decoded = self.decode(&current)?;
            for instruction in &decoded.instructions {
                if instruction.opcode.name != "definefunc" {
                    continue;
                }
                let target = instruction.entity(1, EntityKind::Method)?;
                if seen.contains(&target) {
                    continue;
                }
                let nested = self.method(target)?;
                if nested.record != method.record {
                    return Err(unsupported("function definition crosses ABC records"));
                }
                self.check_storage::<SourceUnit<'a>>(seen.len() + 1)?;
                seen.try_reserve(1)
                    .map_err(|_| resource("ABC definition set allocation failed"))?;
                pending
                    .try_reserve(1)
                    .map_err(|_| resource("ABC definition queue allocation failed"))?;
                seen.insert(target);
                pending.push(nested);
            }
            units
                .try_reserve(1)
                .map_err(|_| resource("ABC definition list allocation failed"))?;
            units.push(SourceUnit {
                method: current,
                decoded,
            });
        }
        let uses_module = units
            .iter()
            .flat_map(|unit| &unit.decoded.instructions)
            .any(|instruction| {
                matches!(
                    instruction.opcode.name,
                    "ldexternalmodulevar"
                        | "wide.ldexternalmodulevar"
                        | "ldlocalmodulevar"
                        | "wide.ldlocalmodulevar"
                        | "stmodulevar"
                        | "wide.stmodulevar"
                        | "getmodulenamespace"
                        | "wide.getmodulenamespace"
                )
            });
        let module = if uses_module {
            self.module(&self.record(method.record)?)?
        } else {
            None
        };
        if uses_module && module.is_none() {
            return Err(invalid("ABC method references missing module metadata"));
        }
        let mut output = Output::new(self.budget.output_bytes);
        for (index, unit) in units.iter().enumerate() {
            let source = self
                .render_unit(unit, &units, module.clone(), index == 0, index != 0)
                .map_err(|mut error| {
                    if index != 0 {
                        error.message = format!(
                            "{} (nested method:0x{:x})",
                            error.message, unit.method.offset
                        );
                    }
                    error
                })?;
            output.write_all(&source)?;
        }
        Ok(output.into_bytes())
    }

    fn render_unit(
        &self,
        unit: &SourceUnit<'a>,
        units: &[SourceUnit<'a>],
        module: Option<Arc<Module>>,
        imports: bool,
        force_capture: bool,
    ) -> Result<Vec<u8>> {
        let method = &unit.method;
        let decoded = &unit.decoded;
        let call_type = self.method_call_type(method)?;
        let have_func = call_type & 8 != 0;
        let have_new_target = call_type & 2 != 0;
        let have_this = call_type & 1 != 0;
        let implicit = u32::from(have_func) + u32::from(have_new_target) + u32::from(have_this);
        let count = decoded
            .code
            .args
            .checked_sub(implicit)
            .ok_or_else(|| invalid("ABC call type exceeds the argument frame"))?;
        self.check_storage::<u32>(count as usize)?;
        let kind = (method.flags >> 8) & 0xff;
        if !matches!(kind, 0 | 1 | 2 | 4 | 6) {
            return Err(unsupported(format!(
                "ABC function kind {kind:#x} needs additional reconstruction"
            )));
        }
        let arrow = matches!(kind, 2 | 6);
        let graph = Graph::build(decoded, self.budget)?;
        let try_regions = simple_tries(decoded, &graph)?;
        let async_plan = crate::async_plan::simple_async(self, decoded)?;
        if async_plan.is_some() != matches!(kind, 4 | 6) {
            return Err(unsupported(
                "async function kind and state machine require matching reconstruction",
            ));
        }
        let arrow_plan = if arrow {
            Some(crate::arrow_plan::arrow_plan(self, decoded, call_type)?)
        } else {
            None
        };
        let ordinary_receivers = crate::fresh::ordinary_receivers(self, decoded, &graph)?;
        let uses_env = decoded.instructions.iter().any(|instruction| {
            matches!(
                instruction.opcode.name,
                "newlexenv"
                    | "wide.newlexenv"
                    | "newlexenvwithname"
                    | "wide.newlexenvwithname"
                    | "ldlexvar"
                    | "wide.ldlexvar"
                    | "stlexvar"
                    | "wide.stlexvar"
                    | "poplexenv"
                    | "definefunc"
            )
        });
        let uses_context = force_capture
            || uses_env
            || decoded
                .instructions
                .iter()
                .any(|instruction| instruction.opcode.name == "ldhole");
        let mut visited = Vec::new();
        visited
            .try_reserve_exact(graph.blocks.len())
            .map_err(|_| resource("ABC region allocation failed"))?;
        visited.resize(graph.blocks.len(), false);
        let mut registers = Vec::new();
        for instruction in &decoded.instructions {
            if arrow_plan
                .as_ref()
                .is_some_and(|plan| plan.skips(instruction.pc))
            {
                continue;
            }
            for operand in &instruction.operands {
                if let Operand::Register(register) = operand {
                    registers
                        .try_reserve(1)
                        .map_err(|_| resource("ABC register set allocation failed"))?;
                    registers.push(*register);
                }
            }
            if let Some(range) = crate::instructions::register_range(instruction)? {
                let count = usize::try_from(range.end - range.start)
                    .map_err(|_| resource("ABC register range overflow"))?;
                self.check_storage::<u32>(count)?;
                registers
                    .try_reserve(count)
                    .map_err(|_| resource("ABC register range allocation failed"))?;
                registers.extend(range.map(|register| register as u32));
            }
        }
        registers.sort_unstable();
        registers.dedup();
        let mut writer = Writer {
            abc: self,
            decoded,
            graph,
            visited,
            ordinary_receivers,
            arrow_plan,
            async_plan,
            try_regions,
            in_try: false,
            uses_context,
            module,
            out: Output::new(self.budget.output_bytes),
        };
        if imports {
            writer.imports(
                units
                    .iter()
                    .flat_map(|unit| unit.decoded.instructions.iter()),
            )?;
        }
        writer.uses_context |= writer.decoded.instructions.iter().any(|instruction| {
            instruction.opcode.name == "throw.undefinedifholewithname"
                && !writer.module_guard(instruction)
        });
        let uses_context = writer.uses_context;
        let named_value = arrow || kind == 0;
        let depth = if uses_context {
            writeln!(
                writer.out,
                "function _abc_capture_{:x}(__abc_context) {{",
                method.offset
            )?;
            writer.out.write_all(if named_value {
                b"    const "
            } else {
                b"    return "
            })?;
            2
        } else {
            if named_value {
                writer.out.write_all(b"const ")?;
            }
            1
        };
        if arrow {
            write!(
                writer.out,
                "_abc_{:x} = {}(",
                method.offset,
                if kind == 6 { "async " } else { "" }
            )?;
        } else if kind == 0 {
            write!(
                writer.out,
                "_abc_{:x} = ({{ _abc_{:x}(",
                method.offset, method.offset
            )?;
        } else {
            write!(
                writer.out,
                "{}function _abc_{:x}(",
                if kind == 4 { "async " } else { "" },
                method.offset
            )?;
        }
        for index in 0..count {
            if index > 0 {
                writer.out.write_all(b", ")?;
            }
            write!(writer.out, "p{index}")?;
        }
        writer
            .out
            .write_all(if arrow { b") => {\n" } else { b") {\n" })?;
        writer.line(depth, "\"use strict\";")?;
        writer.line(depth, "let acc;")?;
        if uses_env {
            writer.line(depth, "let __abc_env = __abc_context.env;")?;
        }
        for register in registers {
            writer.indent(depth)?;
            write!(writer.out, "let v{register}")?;
            if register >= decoded.code.vregs {
                let mut arg = register - decoded.code.vregs;
                if have_func && arg == 0 {
                    write!(writer.out, " = _abc_{:x}", method.offset)?;
                } else {
                    arg -= u32::from(have_func);
                    if have_new_target && arg == 0 {
                        writer.out.write_all(if arrow {
                            b" = void 0"
                        } else {
                            b" = new.target"
                        })?;
                    } else {
                        arg -= u32::from(have_new_target);
                        if have_this && arg == 0 {
                            if arrow {
                                return Err(unsupported("arrow invocation-frame this escapes"));
                            }
                            writer.out.write_all(b" = this")?;
                        } else {
                            arg -= u32::from(have_this);
                            write!(writer.out, " = p{arg}")?;
                        }
                    }
                }
            }
            writer.out.write_all(b";\n")?;
        }
        let mut prefix = Vec::new();
        prefix
            .try_reserve_exact(writer.out.len())
            .map_err(|_| resource("ABC source prefix allocation failed"))?;
        prefix.extend_from_slice(&writer.out);
        let end = writer.graph.blocks.len();
        let structured = writer.range(0, end, end, None, depth).and_then(|()| {
            if writer
                .graph
                .reachable
                .iter()
                .zip(&writer.visited)
                .any(|(reachable, visited)| *reachable && !visited)
            {
                return Err(unsupported("reachable ABC blocks were not reconstructed"));
            }
            Ok(())
        });
        if let Err(error) = structured {
            if error.code != "DECOMPILE_FAILED" || !writer.try_regions.is_empty() {
                return Err(error);
            }
            writer.out = Output::new(self.budget.output_bytes);
            writer.out.write_all(&prefix)?;
            writer.visited.fill(false);
            writer.forward_labels(depth)?;
        }
        if kind == 0 {
            if uses_context {
                writer.out.write_all(b"    } }).")?;
            } else {
                writer.out.write_all(b"} }).")?;
            }
            writeln!(writer.out, "_abc_{:x};", method.offset)?;
        } else if arrow {
            writer
                .out
                .write_all(if uses_context { b"    };\n" } else { b"};\n" })?;
        } else {
            writer
                .out
                .write_all(if uses_context { b"    };\n" } else { b"}\n" })?;
        }
        if uses_context {
            if named_value {
                writeln!(writer.out, "    return _abc_{:x};", method.offset)?;
            }
            writer.out.write_all(b"}\n")?;
        }
        Ok(writer.out.into_bytes())
    }
}
