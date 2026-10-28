use crate::{
    Abc, Coordinate, Decoded, EntityKind, Instruction, LiteralValue, Method, Module, Name, Operand,
    class_plan::ClassPlan,
    reader::{invalid, resource},
    structure::{Graph, unsupported},
};
use rxd_core::{Error, Result, output::Output};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    sync::Arc,
};

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
    start_pc: u32,
    handler_pc: u32,
    join_pc: u32,
}

#[derive(Clone, Copy, Default)]
struct Implicit {
    func: Option<u32>,
    new_target: Option<u32>,
    this_reg: Option<u32>,
    args: Option<u32>,
    params: u32,
}

struct Writer<'a, 'b> {
    abc: &'a Abc<'b>,
    decoded: &'a Decoded<'b>,
    graph: &'a Graph,
    visited: Vec<bool>,
    ordinary_receivers: &'a [bool],
    async_plan: Option<&'a crate::async_plan::AsyncPlan>,
    try_regions: &'a [TryRegion],
    uses_context: bool,
    module: Option<Arc<Module>>,
    implicit: Implicit,
    function_ref: Option<&'a str>,
    func_unused: bool,
    classes: &'a HashMap<u32, ClassUse>,
    super_context: bool,
    arrow: bool,
    top_level_arrow: bool,
    module_body: bool,
    generator: bool,
    dominance: Vec<Option<usize>>,
    escape_serial: usize,
    escapes: Vec<(u32, String)>,
    out: Output,
}

struct RenderContext<'a> {
    module: Option<Arc<Module>>,
    classes: &'a HashMap<u32, ClassUse>,
}

struct FunctionValueOptions<'a> {
    inline: bool,
    module: Option<Arc<Module>>,
    classes: &'a HashMap<u32, ClassUse>,
    top_level_arrow: bool,
}

struct BodyOptions<'a> {
    function_ref: Option<&'a str>,
    super_context: bool,
    top_level_arrow: bool,
    module: Option<Arc<Module>>,
    classes: &'a HashMap<u32, ClassUse>,
}

#[derive(Clone, Copy)]
struct ClassUse {
    constructor: u32,
    has_base: bool,
}

const CLASS_REGISTRY: &str = concat!(
    "const __abc_class_registry = globalThis[Symbol.for(\"rxd.abc.class-registry\")] || ",
    "(globalThis[Symbol.for(\"rxd.abc.class-registry\")] = {prototypes: new WeakSet(), constructors: new WeakSet()});\n",
    "const __abc_class_prototypes = __abc_class_registry.prototypes;\n",
    "const __abc_class_constructors = __abc_class_registry.constructors;\n",
);

const COPY_HELPER: &str = "function __abc_copy(d,s){if(s!=null){const f=Object(s);for(const k of Reflect.ownKeys(f)){const x=Object.getOwnPropertyDescriptor(f,k);if(x&&x.enumerable)Reflect.defineProperty(d,k,\"value\"in x?{...x,writable:true,configurable:true}:{...x,configurable:true})}}return d}\n";

fn validate_generator_resume(instructions: &[Instruction]) -> Result<()> {
    for (index, instruction) in instructions.iter().enumerate() {
        if instruction.opcode.name != "getresumemode" {
            continue;
        }
        let dispatch = index >= 3
            && instructions[index - 3].opcode.name == "resumegenerator"
            && instructions[index - 2].opcode.name == "sta"
            && instructions[index - 1].opcode.name == "lda"
            && instructions
                .get(index + 1)
                .is_some_and(|next| next.opcode.name == "sta")
            && match instructions.get(index + 2) {
                Some(next) if next.opcode.name == "ldai" => {
                    matches!(next.operands.first(), Some(Operand::Signed(0..=2)))
                        && instructions
                            .get(index + 3)
                            .is_some_and(|next| next.opcode.name == "eq")
                }
                Some(next) if next.opcode.name == "jmp" => true,
                _ => false,
            };
        if !dispatch {
            return Err(unsupported(format!(
                "generator resume mode at PC {:#x} is not a compiler dispatch",
                instruction.pc
            )));
        }
    }
    Ok(())
}

fn identifier(name: &Name) -> Option<&str> {
    identifier_text(name.scalar_text()?)
}

fn identifier_text(text: &str) -> Option<&str> {
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
            self.out.write_all(b"\t")?;
        }
        Ok(())
    }

    fn line(&mut self, depth: usize, text: &str) -> Result<()> {
        if text == "void 0;" {
            return Ok(());
        }
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
        self.out.write_all(if object { b"{" } else { b"[" })?;
        let mut index = 0;
        let mut first = true;
        while index < literals.len() {
            if !first {
                self.out.write_all(b", ")?;
            }
            first = false;
            if object {
                let LiteralValue::String(key) = &literals[index].value else {
                    return Err(unsupported("object literal has a non-string key"));
                };
                self.out.write_all(b"[")?;
                key.write_quoted(&mut self.out)?;
                self.out.write_all(b"]: ")?;
                index += 1;
                if index >= literals.len() {
                    return Err(unsupported("object literal key has no value"));
                }
                if let LiteralValue::Method(target) = literals[index].value {
                    let size = literals
                        .get(index + 1)
                        .and_then(|literal| match &literal.value {
                            LiteralValue::Integer(size) => Some(*size),
                            LiteralValue::Affiliate(size) => Some(u32::from(*size)),
                            _ => None,
                        });
                    if let Some(size) = size {
                        self.out.write_all(b"Object.defineProperty(")?;
                        self.method_value(target)?;
                        write!(self.out, ", \"length\", {{value: {size}}})")?;
                        index += 2;
                    } else {
                        self.method_value(target)?;
                        index += 1;
                    }
                } else {
                    let literal = &literals[index];
                    match (&literal.tag, &literal.value) {
                        (1, LiteralValue::Byte(_)) | (2..=5, _) => self.literal(&literal.value)?,
                        (0xff, LiteralValue::Byte(_)) => self.out.write_all(b"null")?,
                        _ => {
                            return Err(unsupported(
                                "literal array value needs method, accessor or nested-literal reconstruction",
                            ));
                        }
                    }
                    index += 1;
                }
            } else {
                let literal = &literals[index];
                match (&literal.tag, &literal.value) {
                    (_, LiteralValue::Method(target)) => self.method_value(*target)?,
                    (0xff, LiteralValue::Byte(_)) => self.out.write_all(b"null")?,
                    (1, LiteralValue::Byte(_)) | (2..=5, _) => self.literal(&literal.value)?,
                    _ => {
                        return Err(unsupported(
                            "literal array value needs method or scalar reconstruction",
                        ));
                    }
                }
                index += 1;
            }
        }
        self.out.write_all(if object { b"}" } else { b"]" })?;
        Ok(())
    }

    fn method_value(&mut self, target: u32) -> Result<()> {
        let new_target = self.new_target_expression();
        write!(
            self.out,
            "_abc_capture_{target:x}({{env: __abc_env, hole: __abc_context.hole, newTarget: {new_target}}})"
        )?;
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

    fn local(&self, index: u32) -> Result<String> {
        let entry = self
            .module()?
            .local_exports
            .get(index as usize)
            .ok_or_else(|| invalid("ABC local module slot is outside module metadata"))?;
        Ok(module_binding(&entry.local_name, index))
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

    fn args(&mut self, registers: impl Iterator<Item = u32>) -> Result<()> {
        for (index, register) in registers.enumerate() {
            if index > 0 {
                self.out.write_all(b", ")?;
            }
            let expression = self.register_text(register)?;
            self.out.write_all(expression.as_bytes())?;
        }
        Ok(())
    }

    fn new_target_expression(&self) -> &'static str {
        if self.module_body {
            "void 0"
        } else if self.arrow {
            if self.top_level_arrow {
                "void 0"
            } else {
                "__abc_context.newTarget"
            }
        } else {
            "new.target"
        }
    }

    fn parameter_name(&self, register: u32) -> Option<String> {
        let base = self.implicit.args?;
        (register >= base && register < base + self.implicit.params)
            .then(|| format!("p{}", register - base))
    }

    fn register_expression(&self, register: u32) -> Option<String> {
        if Some(register) == self.implicit.this_reg {
            Some("__abc_this".to_owned())
        } else if Some(register) == self.implicit.new_target {
            Some(self.new_target_expression().to_owned())
        } else if let Some(base) = self.implicit.args
            && register >= base
            && register < base + self.implicit.params
        {
            Some(format!("p{}", register - base))
        } else {
            None
        }
    }

    fn register_text(&self, register: u32) -> Result<String> {
        if let Some(expression) = self.register_expression(register) {
            Ok(expression)
        } else if Some(register) == self.implicit.func {
            Err(unsupported("method observes its own function object"))
        } else {
            Ok(format!("v{register}"))
        }
    }

    fn emit_instruction(&mut self, instruction: &Instruction, depth: usize) -> Result<()> {
        let name = instruction.opcode.name;
        if self.generator {
            match name {
                "creategeneratorobj" => return self.line(depth, "acc = void 0;"),
                "suspendgenerator" => return Ok(()),
                "resumegenerator" => return self.line(depth, "acc = __abc_resume;"),
                "getresumemode" => return self.line(depth, "acc = 0;"),
                "createiterresultobj" => {
                    let value = self.register_text(instruction.register(1)?)?;
                    self.indent(depth)?;
                    writeln!(self.out, "__abc_resume = yield {value};")?;
                    return Ok(());
                }
                _ => {}
            }
        }
        if let Some(plan) = &self.async_plan {
            if instruction.pc == plan.enter
                || instruction.pc == plan.save
                || instruction.pc == plan.reject_return
                || name == "asyncfunctionresolve"
                || name == "suspendgenerator"
            {
                return Ok(());
            }
            if name == "asyncfunctionreject" {
                return self.line(depth, "throw acc;");
            }
            if name == "asyncfunctionawaituncaught" {
                self.indent(depth)?;
                self.out.write_all(b"__abc_await = await acc;\n")?;
                return Ok(());
            }
            if name == "resumegenerator" {
                self.indent(depth)?;
                self.out.write_all(b"acc = __abc_await;\n")?;
                return Ok(());
            }
            if name == "getresumemode" {
                self.indent(depth)?;
                self.out.write_all(b"acc = 0;\n")?;
                return Ok(());
            }
        }
        if self.module_guard(instruction) {
            return Ok(());
        }
        match name {
            "nop" | "callruntime.notifyconcurrentresult" => return Ok(()),
            "return" if self.module_body => return self.line(depth, "void acc;"),
            "returnundefined" if self.module_body => return self.line(depth, "void 0;"),
            "return" => return self.line(depth, "return acc;"),
            "returnundefined" => return self.line(depth, "return;"),
            "throw" => return self.line(depth, "throw acc;"),
            "throw.notexists" => {
                return self.line(
                    depth,
                    "throw new TypeError(\"Throw method is not defined\");",
                );
            }
            "throw.patternnoncoercible" => {
                return self.line(depth, "throw new TypeError(\"object is not coercible\");");
            }
            _ => {}
        }
        self.indent(depth)?;
        match name {
            "lda" => {
                let source = instruction.register(0)?;
                if let Some(expression) = self.register_expression(source) {
                    write!(self.out, "acc = {expression}")?;
                } else if Some(source) == self.implicit.func {
                    if let Some(reference) = self.function_ref {
                        write!(self.out, "acc = {reference}")?;
                    } else if self.func_unused {
                        self.out.write_all(b"void 0")?;
                    } else {
                        return Err(missing(instruction));
                    }
                } else {
                    write!(self.out, "acc = v{source}")?;
                }
            }
            "sta" => {
                let dest = instruction.register(0)?;
                if Some(dest) == self.implicit.this_reg {
                    self.out.write_all(b"__abc_this = acc")?;
                } else if let Some(parameter) = self.parameter_name(dest) {
                    write!(self.out, "{parameter} = acc")?;
                } else if dest == self.implicit.func.unwrap_or(u32::MAX)
                    || dest == self.implicit.new_target.unwrap_or(u32::MAX)
                    || self.register_expression(dest).is_some()
                {
                    return Err(missing(instruction));
                } else {
                    write!(self.out, "v{dest} = acc")?;
                }
            }
            "mov" => {
                let dest = instruction.register(0)?;
                let source = instruction.register(1)?;
                if Some(dest) == self.implicit.this_reg {
                    write!(self.out, "__abc_this = v{source}")?;
                } else if let Some(parameter) = self.parameter_name(dest) {
                    write!(self.out, "{parameter} = v{source}")?;
                } else if dest == self.implicit.func.unwrap_or(u32::MAX)
                    || Some(dest) == self.implicit.new_target
                    || self.register_expression(dest).is_some()
                {
                    return Err(missing(instruction));
                } else if let Some(expression) = self.register_expression(source) {
                    write!(self.out, "v{dest} = {expression}")?;
                } else if Some(source) == self.implicit.func {
                    if let Some(reference) = self.function_ref {
                        write!(self.out, "v{dest} = {reference}")?;
                    } else if self.func_unused {
                        self.out.write_all(b"void 0")?;
                    } else {
                        return Err(unsupported(format!(
                            "method observes its own function object at PC {:#x}",
                            instruction.pc
                        )));
                    }
                } else {
                    write!(self.out, "v{dest} = v{source}")?;
                }
            }
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
            "ldthis" => self.out.write_all(b"acc = __abc_this")?,
            "ldnewtarget" => {
                let expression = self.new_target_expression();
                write!(self.out, "acc = {expression}")?;
            }
            "ldfunction" => {
                let reference = self
                    .function_ref
                    .ok_or_else(|| unsupported("method observes its own function object"))?;
                write!(self.out, "acc = {reference}")?;
            }
            "definefunc" => {
                let target = instruction.entity(1, EntityKind::Method)?;
                let new_target = self.new_target_expression();
                writeln!(
                    self.out,
                    "acc = _abc_capture_{target:x}({{env: __abc_env, hole: __abc_context.hole, newTarget: {new_target}}});"
                )?;
                self.indent(depth)?;
                write!(
                    self.out,
                    "Object.defineProperty(acc, \"length\", {{value: {}}})",
                    instruction.unsigned(2)?
                )?;
            }
            "definemethod" => {
                let target = instruction.entity(1, EntityKind::Method)?;
                let new_target = self.new_target_expression();
                write!(
                    self.out,
                    "acc = _abc_capture_{target:x}({{env: __abc_env, hole: __abc_context.hole, newTarget: {new_target}}})"
                )?;
            }
            "defineclasswithbuffer" => {
                let class = self
                    .classes
                    .get(&instruction.pc)
                    .ok_or_else(|| invalid("ABC class definition was not planned"))?;
                let constructor = class.constructor;
                let new_target = self.new_target_expression();
                write!(
                    self.out,
                    "acc = _abc_class_{constructor:x}({{env: __abc_env, hole: __abc_context.hole, newTarget: {new_target}}}"
                )?;
                if class.has_base {
                    let proto = self.register_text(instruction.register(4)?)?;
                    write!(self.out, ", {proto}")?;
                }
                self.out.write_all(b")")?;
            }
            "throw.ifsupernotcorrectcall" => {
                match instruction.unsigned(0)? {
                    0 => self.out.write_all(
                        b"if (acc === void 0) { throw new ReferenceError(\"sub-class must call super before use 'this'\"); }",
                    )?,
                    1 => self.out.write_all(
                        b"if (acc !== void 0) { throw new ReferenceError(\"super() forbidden re-bind 'this'\"); }",
                    )?,
                    _ => return Err(invalid("unknown ABC super initialization check")),
                }
            }
            "supercallthisrange" | "supercallarrowrange" => {
                if !self.super_context {
                    return Err(missing(instruction));
                }
                let count = instruction.unsigned(1)?;
                let base = instruction.register(2)?;
                self.out.write_all(b"acc = super(")?;
                self.args(base..base + count)?;
                self.out.write_all(b")")?;
            }
            "wide.supercallthisrange" | "wide.supercallarrowrange" => {
                if !self.super_context {
                    return Err(missing(instruction));
                }
                let count = instruction.unsigned(0)?;
                let base = instruction.register(1)?;
                self.out.write_all(b"acc = super(")?;
                self.args(base..base + count)?;
                self.out.write_all(b")")?;
            }
            "supercallspread" => {
                if !self.super_context {
                    return Err(missing(instruction));
                }
                let source = self.register_text(instruction.register(1)?)?;
                write!(self.out, "acc = super(...{source})")?;
            }
            "callruntime.supercallforwardallargs" => {
                if !self.super_context {
                    return Err(missing(instruction));
                }
                self.out.write_all(b"acc = super(...arguments)")?;
            }
            "ldsuperbyname" => {
                if !self.super_context {
                    return Err(missing(instruction));
                }
                self.out.write_all(b"acc = super[")?;
                self.literal_string(instruction, 1)?;
                self.out.write_all(b"]")?;
            }
            "stsuperbyname" => {
                if !self.super_context {
                    return Err(missing(instruction));
                }
                self.out.write_all(b"super[")?;
                self.literal_string(instruction, 1)?;
                self.out.write_all(b"] = acc")?;
            }
            "ldsuperbyvalue" => {
                if !self.super_context {
                    return Err(missing(instruction));
                }
                let key = self.register_text(instruction.register(1)?)?;
                write!(self.out, "acc = super[{key}]")?;
            }
            "stsuperbyvalue" => {
                if !self.super_context {
                    return Err(missing(instruction));
                }
                let key = self.register_text(instruction.register(2)?)?;
                write!(self.out, "super[{key}] = acc")?;
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
                let binding = self.local(instruction.unsigned(0)?)?;
                if name.contains("stmodulevar") {
                    write!(self.out, "{binding} = acc")?;
                } else {
                    write!(self.out, "acc = {binding}")?;
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
                let left = self.register_text(instruction.register(1)?)?;
                write!(self.out, "acc = {left} {operator} acc")?;
            }
            "ldobjbyname" => {
                self.out.write_all(b"acc = acc[")?;
                self.literal_string(instruction, 1)?;
                self.out.write_all(b"]")?;
            }
            "stobjbyname" => {
                let object = self.register_text(instruction.register(2)?)?;
                write!(self.out, "{object}[")?;
                self.literal_string(instruction, 1)?;
                self.out.write_all(b"] = acc")?;
            }
            "ldobjbyvalue" => {
                let object = self.register_text(instruction.register(1)?)?;
                write!(self.out, "acc = {object}[acc]")?;
            }
            "stobjbyvalue" => {
                let object = self.register_text(instruction.register(1)?)?;
                let key = self.register_text(instruction.register(2)?)?;
                write!(self.out, "{object}[{key}] = acc")?;
            }
            "ldobjbyindex" | "wide.ldobjbyindex" => {
                write!(
                    self.out,
                    "acc = acc[{}]",
                    instruction.unsigned(usize::from(name == "ldobjbyindex"))?
                )?;
            }
            "stobjbyindex" | "wide.stobjbyindex" => {
                let wide = name.starts_with("wide.");
                let object = self.register_text(instruction.register(usize::from(!wide))?)?;
                write!(
                    self.out,
                    "{object}[{}] = acc",
                    instruction.unsigned(if wide { 1 } else { 2 })?
                )?;
            }
            "definefieldbyname" | "definepropertybyname" => {
                let object = self.register_text(instruction.register(2)?)?;
                write!(self.out, "Object.defineProperty({object}, ")?;
                self.literal_string(instruction, 1)?;
                self.out.write_all(
                    b", {value: acc, writable: true, enumerable: true, configurable: true})",
                )?;
            }
            "stownbyname" | "stownbyvalue" | "stownbyindex" | "wide.stownbyindex" => {
                let object = if name == "stownbyname" {
                    instruction.register(2)?
                } else if name == "wide.stownbyindex" {
                    instruction.register(0)?
                } else {
                    instruction.register(1)?
                };
                let object = self.register_text(object)?;
                write!(self.out, "Object.defineProperty({object}, ")?;
                match name {
                    "stownbyname" => self.literal_string(instruction, 1)?,
                    "stownbyvalue" => {
                        let key = self.register_text(instruction.register(2)?)?;
                        self.out.write_all(key.as_bytes())?;
                    }
                    "stownbyindex" => write!(self.out, "{}", instruction.unsigned(2)?)?,
                    _ => write!(self.out, "{}", instruction.unsigned(1)?)?,
                }
                let proven = self
                    .decoded
                    .instructions
                    .binary_search_by_key(&instruction.pc, |item| item.pc)
                    .ok()
                    .and_then(|index| self.ordinary_receivers.get(index))
                    .copied()
                    .unwrap_or(false);
                if proven {
                    self.out.write_all(
                        b", {value: acc, writable: true, enumerable: true, configurable: true})",
                    )?;
                } else {
                    self.out.write_all(
                        b", {value: acc, writable: true, configurable: true, enumerable: !(__abc_class_prototypes.has(",
                    )?;
                    self.out.write_all(object.as_bytes())?;
                    self.out.write_all(b") || __abc_class_constructors.has(")?;
                    self.out.write_all(object.as_bytes())?;
                    self.out.write_all(b"))})")?;
                }
            }
            "callruntime.definefieldbyvalue" => {
                let object = self.register_text(instruction.register(2)?)?;
                let key = self.register_text(instruction.register(1)?)?;
                write!(
                    self.out,
                    "Object.defineProperty({object}, {key}, {{value: acc, writable: true, enumerable: true, configurable: true}})"
                )?;
            }
            "callruntime.definefieldbyindex" => {
                let object = self.register_text(instruction.register(2)?)?;
                write!(
                    self.out,
                    "Object.defineProperty({object}, {}, {{value: acc, writable: true, enumerable: true, configurable: true}})",
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
                let receiver = self.register_text(instruction.register(1)?)?;
                write!(self.out, "acc = Reflect.apply(acc, {receiver}, [")?;
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
                    let receiver = self.register_text(base)?;
                    write!(self.out, "acc = Reflect.apply(acc, {receiver}, [")?;
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
                let constructor = self.register_text(base)?;
                write!(self.out, "acc = new {constructor}(")?;
                self.args((base + 1)..(base + count))?;
                self.out.write_all(b")")?;
            }
            "tonumeric" => self
                .out
                .write_all(b"acc = (typeof acc === \"bigint\" ? acc : +acc)")?,
            "delobjprop" => {
                let object = self.register_text(instruction.register(0)?)?;
                write!(self.out, "delete {object}[acc]")?;
            }
            "getpropiterator" => self
                .out
                .write_all(b"acc = Object.keys(acc)[Symbol.iterator]()")?,
            "getiterator" => self.out.write_all(b"acc = acc[Symbol.iterator]()")?,
            "getnextpropname" => {
                let iterator = self.register_text(instruction.register(0)?)?;
                write!(self.out, "acc = {iterator}.next().value")?;
            }
            "copydataproperties" => {
                let target = self.register_text(instruction.register(0)?)?;
                write!(self.out, "acc = __abc_copy({target}, acc)")?;
            }
            "starrayspread" => {
                let target = self.register_text(instruction.register(0)?)?;
                let index = self.register_text(instruction.register(1)?)?;
                write!(self.out, "{target}.splice({index}, 0, ...acc)")?;
            }
            "definegettersetterbyvalue" => {
                let object = self.register_text(instruction.register(0)?)?;
                let property = self.register_text(instruction.register(1)?)?;
                let getter = self.register_text(instruction.register(2)?)?;
                let setter = self.register_text(instruction.register(3)?)?;
                write!(
                    self.out,
                    "Object.defineProperty({object}, {property}, {{get: {getter}, set: {setter}, enumerable: true, configurable: true}})"
                )?;
            }
            "setobjectwithproto" => {
                let proto = self.register_text(instruction.register(1)?)?;
                write!(self.out, "Object.setPrototypeOf(acc, {proto})")?;
            }
            "apply" => {
                let receiver = self.register_text(instruction.register(1)?)?;
                let arguments = self.register_text(instruction.register(2)?)?;
                write!(self.out, "acc = Reflect.apply(acc, {receiver}, {arguments})")?;
            }
            "throw.ifnotobject" => {
                let value = self.register_text(instruction.register(0)?)?;
                write!(
                    self.out,
                    "if ({value} === null || (typeof {value} !== \"object\" && typeof {value} !== \"function\")) {{ throw new TypeError(\"not an object\"); }}"
                )?;
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

    fn dominates(&self, header: usize, block: usize) -> bool {
        if header == block {
            return true;
        }
        let mut current = block;
        let mut steps = 0usize;
        while let Some(next) = self.dominance.get(current).copied().flatten() {
            if next == header {
                return true;
            }
            if next == current || steps > self.dominance.len() {
                return false;
            }
            current = next;
            steps += 1;
        }
        false
    }

    fn compute_loops(&self) -> Result<Vec<(usize, usize)>> {
        let mut loops: Vec<(usize, usize)> = Vec::new();
        for (index, block) in self.graph.blocks.iter().enumerate() {
            if !self.graph.reachable[index] {
                continue;
            }
            let last = &self.decoded.instructions[block.end - 1];
            if last.branch.is_some() {
                let target = self.jump_target(last)?;
                if target <= index && self.dominates(target, index) {
                    match loops.iter_mut().find(|(header, _)| *header == target) {
                        Some(entry) => entry.1 = entry.1.max(index),
                        None => loops.push((target, index)),
                    }
                }
            }
        }
        loops.sort_unstable();
        for (index, outer) in loops.iter().enumerate() {
            for inner in &loops[index + 1..] {
                if inner.0 <= outer.1 && outer.1 < inner.1 {
                    return Err(unsupported("overlapping loops need region analysis"));
                }
            }
        }
        Ok(loops)
    }

    fn block_pc(&self, target: usize) -> u32 {
        if target >= self.graph.blocks.len() {
            self.decoded.code.bytes.len() as u32
        } else {
            self.decoded.instructions[self.graph.blocks[target].first].pc
        }
    }

    fn label_name(&self, target: usize) -> String {
        format!(
            "__abc_goto_{:x}_{}",
            self.block_pc(target),
            self.escape_serial
        )
    }

    fn linear(&mut self, base_depth: usize) -> Result<()> {
        let successors = crate::structure::successors(self.decoded, self.graph)?;
        let dominance = crate::dominators::compute_for(self.abc, &successors, 0)?;
        self.dominance = dominance.idom;
        let loops = self.compute_loops()?;
        let end = self.graph.blocks.len();
        self.linear_region(0..end, &loops, None, None, &[], base_depth)
    }

    fn linear_region(
        &mut self,
        range: std::ops::Range<usize>,
        loops: &[(usize, usize)],
        active: Option<Loop>,
        entered_try: Option<usize>,
        escapes: &[(u32, String)],
        depth: usize,
    ) -> Result<()> {
        let (start, end) = (range.start, range.end);
        if depth > self.abc.budget.ast_depth {
            return Err(resource("ABC source nesting exceeds depth guard"));
        }
        let inner: Vec<(usize, usize)> = loops
            .iter()
            .copied()
            .filter(|(header, latch)| *header >= start && *latch < end)
            .filter(|(header, _)| active.is_none_or(|loop_| loop_.header != *header))
            .collect();
        let tries: Vec<usize> = self
            .try_regions
            .iter()
            .enumerate()
            .filter(|(index, region)| {
                region.start >= start && region.join <= end && Some(*index) != entered_try
            })
            .map(|(index, _)| index)
            .collect();
        let inside_loop = |index: usize| {
            inner
                .iter()
                .any(|(header, latch)| *header < index && index <= *latch)
        };
        let inside_try = |index: usize| {
            tries.iter().any(|try_index| {
                let region = self.try_regions[*try_index];
                region.start < index && index < region.join
            })
        };
        let mut targets = Vec::new();
        for index in start..end {
            if !self.graph.reachable[index] {
                continue;
            }
            let last = &self.decoded.instructions[self.graph.blocks[index].end - 1];
            if last.branch.is_some() {
                let target = self.jump_target(last)?;
                if target >= start && target < end && !inside_loop(target) && !inside_try(target) {
                    targets.push(target);
                }
            }
        }
        targets.sort_unstable();
        targets.dedup();
        targets.retain(|target| Some(*target) != active.map(|loop_| loop_.header));
        let label_names: Vec<String> = targets
            .iter()
            .map(|target| self.label_name(*target))
            .collect();
        let mut nested: Vec<(u32, String)> = escapes.to_vec();
        for (index, target) in targets.iter().enumerate() {
            nested.push((self.block_pc(*target), label_names[index].clone()));
        }
        let mut level = depth;
        for index in (0..targets.len()).rev() {
            self.indent(level)?;
            writeln!(self.out, "{}: {{", label_names[index])?;
            level += 1;
        }
        let mut closing = 0usize;
        let mut cursor = start;
        while cursor < end {
            while closing < targets.len() && targets[closing] == cursor {
                level -= 1;
                self.line(level, "}")?;
                closing += 1;
            }
            if !self.graph.reachable[cursor] {
                cursor += 1;
                continue;
            }
            if let Some((_, latch)) = inner.iter().find(|(header, _)| *header == cursor) {
                let latch = *latch;
                self.line(level, "while (true) {")?;
                let exit = latch + 1;
                self.linear_region(
                    cursor..exit,
                    loops,
                    Some(Loop {
                        header: cursor,
                        exit,
                    }),
                    None,
                    &nested,
                    level + 1,
                )?;
                self.line(level, "}")?;
                cursor = exit;
                continue;
            }
            let try_index = tries
                .iter()
                .copied()
                .filter(|index| self.try_regions[*index].start == cursor)
                .max_by_key(|index| self.try_regions[*index].join_pc);
            if let Some(index) = try_index {
                let region = self.try_regions[index];
                self.escape_serial += 1;
                let join_label = format!("__abc_try_{:x}_{}", region.join_pc, self.escape_serial);
                let end_pc = self.block_pc(region.end);
                let extends_body = region.end < region.handler
                    && self
                        .decoded
                        .instructions
                        .iter()
                        .filter(|instruction| {
                            end_pc <= instruction.pc && instruction.pc < region.handler_pc
                        })
                        .all(|instruction| linear_safe(instruction.opcode.name));
                let body_end = if extends_body {
                    region.handler
                } else {
                    region.end
                };
                let has_bypass = region.end < region.handler && !extends_body;
                let end_label = has_bypass.then(|| {
                    format!(
                        "__abc_try_end_{:x}_{}",
                        self.block_pc(region.end),
                        self.escape_serial
                    )
                });
                let mut branch_escapes = nested.clone();
                branch_escapes.push((region.join_pc, join_label.clone()));
                if let Some(end_label) = &end_label {
                    branch_escapes.push((self.block_pc(region.end), end_label.clone()));
                }
                self.indent(level)?;
                writeln!(self.out, "{join_label}: {{")?;
                let try_level = if let Some(end_label) = &end_label {
                    self.indent(level + 1)?;
                    writeln!(self.out, "{end_label}: {{")?;
                    level + 2
                } else {
                    level + 1
                };
                self.indent(try_level)?;
                self.out.write_all(b"try {\n")?;
                self.linear_region(
                    region.start..body_end,
                    loops,
                    active,
                    Some(index),
                    &branch_escapes,
                    try_level + 1,
                )?;
                self.indent(try_level)?;
                self.out.write_all(b"} catch (__abc_exception) {\n")?;
                self.line(try_level + 1, "acc = __abc_exception;")?;
                self.linear_region(
                    region.handler..region.join,
                    loops,
                    active,
                    None,
                    &branch_escapes,
                    try_level + 1,
                )?;
                self.indent(try_level + 1)?;
                writeln!(self.out, "break {join_label};")?;
                self.indent(try_level)?;
                self.out.write_all(b"}\n")?;
                if end_label.is_some() {
                    self.indent(level + 1)?;
                    self.out.write_all(b"}\n")?;
                }
                if has_bypass {
                    self.linear_region(
                        region.end..region.handler,
                        loops,
                        active,
                        None,
                        &branch_escapes,
                        level + 1,
                    )?;
                }
                self.line(level, "}")?;
                cursor = region.join;
                continue;
            }
            if self.visited[cursor] {
                cursor += 1;
                continue;
            }
            self.visited[cursor] = true;
            let block = self.graph.blocks[cursor];
            let last = &self.decoded.instructions[block.end - 1];
            let terminal = last.opcode.is_jump() || crate::instructions::is_terminator(last);
            let body_end = if terminal { block.end - 1 } else { block.end };
            for instruction in &self.decoded.instructions[block.first..body_end] {
                self.emit_instruction(instruction, level)?;
            }
            if crate::instructions::is_terminator(last) {
                self.emit_instruction(last, level)?;
                cursor += 1;
                continue;
            }
            if !last.opcode.is_jump() {
                cursor += 1;
                continue;
            }
            let target = self.jump_target(last)?;
            let target_pc = self.block_pc(target);
            let label = if target_pc > last.pc {
                targets
                    .binary_search(&target)
                    .ok()
                    .map(|index| label_names[index].clone())
                    .or_else(|| {
                        escapes
                            .iter()
                            .rev()
                            .find(|(escape, _)| *escape == target_pc)
                            .map(|(_, label)| label.clone())
                    })
            } else {
                None
            };
            self.indent(level)?;
            if last.opcode.is_conditional() {
                self.out.write_all(b"if (")?;
                self.condition(last)?;
                if active.is_some_and(|loop_| target == loop_.header) {
                    self.out.write_all(b") { continue; }\n")?;
                } else if active.is_some_and(|loop_| target == loop_.exit) {
                    self.out.write_all(b") { break; }\n")?;
                } else if let Some(label) = label {
                    writeln!(self.out, ") {{ break {label}; }}")?;
                } else {
                    return Err(unsupported(format!(
                        "unstructured conditional target {target_pc:#x} from {:#x} in linear fallback",
                        last.pc
                    )));
                }
                cursor += 1;
                continue;
            }
            if active.is_some_and(|loop_| target == loop_.header) {
                self.line(level, "continue;")?;
            } else if active.is_some_and(|loop_| target == loop_.exit) {
                self.line(level, "break;")?;
            } else if let Some(label) = label {
                self.line(level, &format!("break {label};"))?;
            } else {
                return Err(unsupported(format!(
                    "unstructured jump target {target_pc:#x} in linear fallback"
                )));
            }
            cursor += 1;
        }
        while closing < targets.len() {
            level -= 1;
            self.line(level, "}")?;
            closing += 1;
        }
        Ok(())
    }

    fn try_candidate(&self, cursor: usize, end: usize, entered: Option<usize>) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (index, region) in self.try_regions.iter().enumerate() {
            if region.start != cursor || Some(index) == entered || region.join > end {
                continue;
            }
            best = match best {
                None => Some(index),
                Some(current) if entered.is_none() => {
                    if region.join_pc > self.try_regions[current].join_pc {
                        Some(index)
                    } else {
                        Some(current)
                    }
                }
                Some(current) => {
                    if region.join_pc < self.try_regions[current].join_pc {
                        Some(index)
                    } else {
                        Some(current)
                    }
                }
            };
        }
        best
    }

    fn condition_chain(
        &self,
        start: usize,
        target: usize,
        end: usize,
    ) -> Option<(usize, usize, Vec<usize>)> {
        if target >= end {
            return None;
        }
        let mut conditions = Vec::new();
        let mut block = start;
        loop {
            if block >= end {
                return None;
            }
            let range = self.graph.blocks[block];
            let last = &self.decoded.instructions[range.end - 1];
            if !last.opcode.is_conditional() || self.jump_target(last).ok()? != target {
                return None;
            }
            conditions.push(block);
            let next = block + 1;
            if next >= end {
                return None;
            }
            if next == target {
                return None;
            }
            block = next;
            let next_last = &self.decoded.instructions[self.graph.blocks[block].end - 1];
            if !next_last.opcode.is_conditional() || self.jump_target(next_last).ok()? != target {
                break;
            }
        }
        let tail_block = block;
        let tail_last = &self.decoded.instructions[self.graph.blocks[tail_block].end - 1];
        if tail_last.opcode.is_conditional() || !tail_last.opcode.is_jump() {
            return None;
        }
        let then_start = self.jump_target(tail_last).ok()?;
        if then_start <= target || then_start >= end {
            return None;
        }
        if then_start < 2 {
            return None;
        }
        let else_tail = &self.decoded.instructions[self.graph.blocks[then_start - 1].end - 1];
        if else_tail.opcode.is_conditional() || !else_tail.opcode.is_jump() {
            return None;
        }
        let join = self.jump_target(else_tail).ok()?;
        if join <= then_start || join > end {
            return None;
        }
        Some((then_start, join, conditions))
    }

    fn range(
        &mut self,
        start: usize,
        end: usize,
        leave: usize,
        context: Option<Loop>,
        entered: Option<usize>,
        depth: usize,
    ) -> Result<()> {
        if depth > self.abc.budget.ast_depth {
            return Err(resource("ABC structure depth exceeds recursion guard"));
        }
        let mut cursor = start;
        while cursor < end {
            if let Some(index) = self.try_candidate(cursor, end, entered) {
                let region = self.try_regions[index];
                self.escape_serial += 1;
                let label = format!("__abc_escape_{:x}_{}", region.join_pc, self.escape_serial);
                self.escapes.push((region.join_pc, label.clone()));
                self.indent(depth)?;
                writeln!(self.out, "{label}: {{")?;
                self.indent(depth + 1)?;
                self.out.write_all(b"try {\n")?;
                self.range(
                    region.start,
                    region.end,
                    region.end,
                    context,
                    Some(index),
                    depth + 2,
                )?;
                self.indent(depth + 1)?;
                self.out.write_all(b"} catch (__abc_exception) {\n")?;
                self.line(depth + 2, "acc = __abc_exception;")?;
                self.range(
                    region.handler,
                    region.join,
                    region.join,
                    context,
                    None,
                    depth + 2,
                )?;
                self.indent(depth + 2)?;
                writeln!(self.out, "break {label};")?;
                self.indent(depth + 1)?;
                self.out.write_all(b"}\n")?;
                if region.end < region.handler {
                    self.range(
                        region.end,
                        region.handler,
                        region.handler,
                        context,
                        None,
                        depth + 1,
                    )?;
                }
                self.escapes.pop();
                self.line(depth, "}")?;
                cursor = region.join;
                continue;
            }
            if !self.graph.reachable[cursor] {
                cursor += 1;
                continue;
            }
            if self.visited[cursor] {
                return Err(unsupported("overlapping ABC source regions"));
            }
            self.visited[cursor] = true;
            let block = self.graph.blocks[cursor];
            let last = &self.decoded.instructions[block.end - 1];
            if last.opcode.is_conditional() {
                let target = self.jump_target(last)?;
                if target > cursor + 1
                    && let Some(chain) = self.condition_chain(cursor, target, end)
                {
                    let (then_start, join, conditions) = chain;
                    self.escape_serial += 1;
                    let flag = format!("__abc_taken_{}", self.escape_serial);
                    self.indent(depth)?;
                    writeln!(self.out, "let {flag} = false;")?;
                    for (index, block_index) in conditions.iter().enumerate() {
                        if index > 0 {
                            self.indent(depth)?;
                            writeln!(self.out, "if (!{flag}) {{")?;
                        }
                        let block = self.graph.blocks[*block_index];
                        self.visited[*block_index] = true;
                        let block_depth = depth + usize::from(index > 0);
                        for instruction in &self.decoded.instructions[block.first..block.end - 1] {
                            self.emit_instruction(instruction, block_depth)?;
                        }
                        let terminator = &self.decoded.instructions[block.end - 1];
                        self.indent(block_depth)?;
                        self.out.write_all(b"if (")?;
                        self.condition(terminator)?;
                        writeln!(self.out, ") {{ {flag} = true; }}")?;
                    }
                    for _ in 1..conditions.len() {
                        self.line(depth, "}")?;
                    }
                    let tail = conditions
                        .last()
                        .copied()
                        .ok_or_else(|| invalid("empty ABC condition chain"))?
                        + 1;
                    self.visited[tail] = true;
                    self.indent(depth)?;
                    writeln!(self.out, "if ({flag}) {{")?;
                    self.range(target, then_start, join, context, None, depth + 1)?;
                    self.line(depth, "} else {")?;
                    self.range(then_start, join, join, context, None, depth + 1)?;
                    self.line(depth, "}")?;
                    cursor = join;
                    continue;
                }
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
                            None,
                            depth + 1,
                        )?;
                        self.line(depth, "}")?;
                        cursor = target;
                        continue;
                    }
                }
            }
            let terminator = last.opcode.is_jump() || crate::instructions::is_terminator(last);
            let body_end = if terminator { block.end - 1 } else { block.end };
            for instruction in &self.decoded.instructions[block.first..body_end] {
                self.emit_instruction(instruction, depth)?;
            }
            if crate::instructions::is_terminator(last) {
                self.emit_instruction(last, depth)?;
                cursor += 1;
                continue;
            }
            if !last.opcode.is_jump() {
                cursor += 1;
                if cursor == end && end != leave {
                    return Err(unsupported("source region falls into a different branch"));
                }
                continue;
            }
            let target = self.jump_target(last)?;
            let target_pc = self.decoded.instructions[self.graph.blocks[target].first].pc;
            if !last.opcode.is_conditional() {
                if let Some(label) = self
                    .escapes
                    .iter()
                    .rev()
                    .find(|(escape, _)| *escape == target_pc)
                    .map(|(_, label)| label.clone())
                {
                    self.indent(depth)?;
                    writeln!(self.out, "break {label};")?;
                    return Ok(());
                }
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
                return Err(unsupported(format!(
                    "unstructured ABC jump target at PC {:#x} targeting {:#x}",
                    last.pc, self.decoded.instructions[self.graph.blocks[target].first].pc
                )));
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
            if let Some(label) = self
                .escapes
                .iter()
                .rev()
                .find(|(escape, _)| *escape == target_pc)
                .map(|(_, label)| label.clone())
            {
                self.indent(depth)?;
                self.out.write_all(b"if (")?;
                self.condition(last)?;
                writeln!(self.out, ") {{ break {label}; }}")?;
                cursor += 1;
                continue;
            }
            if target > end || target == end && leave != end {
                return Err(unsupported(format!(
                    "conditional branch exits its structured region at PC {:#x} targeting {:#x}",
                    last.pc, self.decoded.instructions[self.graph.blocks[target].first].pc
                )));
            }
            let previous = self.graph.blocks[target - 1];
            let tail = &self.decoded.instructions[previous.end - 1];
            let join = if tail.opcode.is_jump() && !tail.opcode.is_conditional() {
                let candidate = self.jump_target(tail)?;
                (candidate > target && candidate <= end).then_some(candidate)
            } else {
                None
            };
            let end_target = join.unwrap_or(target);
            let end_pc = if end_target == self.graph.blocks.len() {
                self.decoded.code.bytes.len() as u32
            } else {
                self.decoded.instructions[self.graph.blocks[end_target].first].pc
            };
            self.escape_serial += 1;
            let label = format!("__abc_escape_{end_pc:x}_{}", self.escape_serial);
            self.indent(depth)?;
            writeln!(self.out, "{label}: {{")?;
            self.escapes.push((end_pc, label));
            self.indent(depth + 1)?;
            self.out.write_all(b"if (!")?;
            self.condition(last)?;
            self.out.write_all(b") {\n")?;
            self.range(cursor + 1, target, end_target, context, None, depth + 2)?;
            if let Some(join) = join {
                self.indent(depth + 1)?;
                self.out.write_all(b"} else {\n")?;
                self.range(target, join, join, context, None, depth + 2)?;
                self.indent(depth + 1)?;
                self.out.write_all(b"}\n")?;
                cursor = join;
            } else {
                self.indent(depth + 1)?;
                self.out.write_all(b"}\n")?;
                cursor = target;
            }
            self.escapes.pop();
            self.line(depth, "}")?;
        }
        Ok(())
    }
}

fn simple_tries(decoded: &Decoded<'_>, graph: &Graph) -> Result<Vec<TryRegion>> {
    let mut raw = Vec::new();
    raw.try_reserve_exact(decoded.code.tries.len())
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
        if handler.type_index != 0 || region.length == 0 {
            return Err(unsupported(
                "typed or unsized exception handler is not yet supported",
            ));
        }
        if handler.length == 0 && pure_rethrow(decoded, handler.pc, 0) {
            continue;
        }
        if handler.length != 0 && pure_rethrow(decoded, handler.pc, handler.length) {
            continue;
        }
        let join_pc = if handler.length == 0 {
            let previous = decoded
                .instructions
                .iter()
                .take_while(|instruction| instruction.end() <= handler.pc)
                .last()
                .ok_or_else(|| invalid("ABC unsized handler has no preceding code"))?;
            if previous.opcode.name == "jmp" && previous.branch == Some(handler.pc) {
                handler.pc
            } else {
                return Err(unsupported(
                    "unsized exception handler needs region analysis",
                ));
            }
        } else {
            handler.pc + handler.length
        };
        let (start_pc, end_pc) = (region.pc, region.pc + region.length);
        let handler_pc = handler.pc;
        if end_pc > handler_pc {
            return Err(unsupported(
                "exception handler overlaps its protected range",
            ));
        }
        let start = block(start_pc)?;
        let end = block(end_pc)?;
        let handler_block = block(handler_pc)?;
        let join = block(join_pc)?;
        if !(start < end && end <= handler_block && handler_block <= join) {
            return Err(unsupported(
                "exception handler does not follow its protected range",
            ));
        }
        raw.push((
            start_pc,
            end_pc,
            handler_pc,
            join_pc,
            start,
            end,
            handler_block,
            join,
        ));
    }
    raw.sort_unstable_by_key(|item| (item.0, std::cmp::Reverse(item.1)));
    for (index, outer) in raw.iter().enumerate() {
        for inner in &raw[index + 1..] {
            if inner.0 < outer.1 {
                if inner.1 > outer.1 || inner.3 > outer.3 {
                    return Err(unsupported(
                        "partially overlapping exception regions need additional analysis",
                    ));
                }
            } else {
                break;
            }
        }
    }
    let mut regions = Vec::new();
    regions
        .try_reserve_exact(raw.len())
        .map_err(|_| resource("ABC exception region allocation failed"))?;
    for (start_pc, end_pc, handler_pc, join_pc, start, end, handler_block, join) in &raw {
        if end_pc < handler_pc {
            let last = decoded
                .instructions
                .iter()
                .take_while(|instruction| instruction.end() <= *handler_pc)
                .last()
                .ok_or_else(|| invalid("ABC exception bypass is empty"))?;
            if last.opcode.name != "jmp" || last.branch != Some(*join_pc) {
                return Err(unsupported(
                    "exception bypass does not jump to the handler end",
                ));
            }
        }
        regions.push(TryRegion {
            start: *start,
            end: *end,
            handler: *handler_block,
            join: *join,
            start_pc: *start_pc,
            handler_pc: *handler_pc,
            join_pc: *join_pc,
        });
    }
    regions.sort_unstable_by_key(|region| (region.start_pc, std::cmp::Reverse(region.join_pc)));
    for instruction in &decoded.instructions {
        let Some(target) = instruction.branch else {
            continue;
        };
        for region in &regions {
            if target >= region.handler_pc
                && target < region.join_pc
                && !(region.handler_pc..region.join_pc).contains(&instruction.pc)
            {
                return Err(unsupported(
                    "normal control flow enters an exception handler",
                ));
            }
        }
    }
    Ok(regions)
}

#[derive(Default)]
struct Scan {
    functions: Vec<Method>,
    classes: Vec<ClassPlan>,
    uses: HashMap<u32, HashMap<u32, ClassUse>>,
    imports: Vec<u32>,
    namespaces: Vec<u32>,
    uses_module: bool,
    class_tracking: bool,
    uses_copy_data: bool,
}

struct Plan<'a> {
    method: Method,
    decoded: Decoded<'a>,
    kind: u8,
    arrow: bool,
    derived: bool,
    uses_super: bool,
    params: u32,
    implicit: Implicit,
    graph: Graph,
    try_regions: Vec<TryRegion>,
    async_plan: Option<crate::async_plan::AsyncPlan>,
    ordinary_receivers: Vec<bool>,
    uses_env: bool,
    uses_context: bool,
    uses_this: bool,
    func_unused: bool,
    generator: bool,
    registers: Vec<u32>,
}

fn indent(output: &mut Output, depth: usize) -> Result<()> {
    for _ in 0..depth {
        output.write_all(b"\t")?;
    }
    Ok(())
}

fn class_name(name: &Name) -> Option<String> {
    let text = name.scalar_text()?;
    let name = text.rsplit_once('#').map_or(text, |(_, tail)| tail);
    identifier(&Name {
        offset: 0,
        end: 0,
        units: name.encode_utf16().collect(),
        text: name.to_owned(),
    })
    .map(str::to_owned)
}

fn element_name(name: &Name) -> String {
    match identifier(name) {
        Some(text) => text.to_owned(),
        None => {
            let mut quoted = Vec::new();
            name.write_quoted(&mut quoted).ok();
            format!("[{}]", String::from_utf8_lossy(&quoted))
        }
    }
}

fn export_name(name: &Name, output: &mut Output) -> Result<()> {
    if name.text == "default" {
        output.write_all(b"default")?;
    } else if identifier(name).is_some() {
        output.write_all(name.text.as_bytes())?;
    } else {
        name.write_quoted(output)?;
    }
    Ok(())
}

fn pure_rethrow(decoded: &Decoded<'_>, handler_pc: u32, handler_len: u32) -> bool {
    let mut cursor = handler_pc;
    let limit = if handler_len == 0 {
        u32::MAX
    } else {
        handler_pc + handler_len
    };
    let mut saved = None;
    let mut steps = 0usize;
    while cursor < limit {
        let Ok(index) = decoded
            .instructions
            .binary_search_by_key(&cursor, |instruction| instruction.pc)
        else {
            return false;
        };
        let instruction = &decoded.instructions[index];
        match instruction.opcode.name {
            "sta" => saved = instruction.register(0).ok(),
            "lda" => {
                if let Some(saved) = saved
                    && instruction.register(0).ok() != Some(saved)
                {
                    return false;
                }
            }
            "poplexenv" | "mov" | "nop" => {}
            "throw" => return true,
            _ => return false,
        }
        cursor = instruction.end();
        steps += 1;
        if steps > 8 {
            return false;
        }
    }
    false
}

fn linear_safe(name: &str) -> bool {
    matches!(
        name,
        "mov"
            | "lda"
            | "sta"
            | "ldai"
            | "ldtrue"
            | "ldfalse"
            | "ldnull"
            | "ldundefined"
            | "ldhole"
            | "ldnan"
            | "ldinfinity"
            | "lda.str"
            | "poplexenv"
            | "newlexenv"
            | "wide.newlexenv"
            | "newlexenvwithname"
            | "wide.newlexenvwithname"
            | "ldlexvar"
            | "wide.ldlexvar"
            | "stlexvar"
            | "wide.stlexvar"
            | "ldexternalmodulevar"
            | "wide.ldexternalmodulevar"
            | "ldlocalmodulevar"
            | "wide.ldlocalmodulevar"
            | "getmodulenamespace"
            | "wide.getmodulenamespace"
            | "jmp"
            | "jeqz"
            | "jnez"
            | "nop"
    )
}

fn module_binding(name: &Name, index: u32) -> String {
    identifier(name)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("__abc_export_{index}"))
}

impl<'a> Abc<'a> {
    pub fn decompile_target(&self, target: Coordinate) -> Result<Vec<u8>> {
        match target {
            Coordinate::Method(offset) => {
                let method = self.method(offset)?;
                self.decompile_method(&method)
            }
            Coordinate::Record(offset) => {
                let record = self.record(offset)?;
                self.decompile_record(&record)
            }
        }
    }

    pub fn decompile_record(&self, record: &crate::Record) -> Result<Vec<u8>> {
        let module = self
            .module(record)?
            .ok_or_else(|| invalid("record has no module metadata"))?;
        let members = self.members(record)?;
        let main = members
            .methods
            .iter()
            .find(|method| method.name.text == "func_main_0" && method.code_offset.is_some())
            .ok_or_else(|| unsupported("record has no module initializer method"))?;
        let scan = self.scan_units(main)?;
        let plan = self.make_plan(main, false)?;
        let mut output = Output::new(self.budget.output_bytes);
        self.write_imports(&module, &scan, &mut output)?;
        if scan.class_tracking {
            output.write_all(CLASS_REGISTRY.as_bytes())?;
        }
        output.write_all(
            b"const __abc_hole = {};\nconst __abc_context = {env: null, hole: __abc_hole, newTarget: void 0};\n",
        )?;
        if scan.uses_copy_data {
            output.write_all(COPY_HELPER.as_bytes())?;
        }
        for (index, export) in module.local_exports.iter().enumerate() {
            writeln!(
                output,
                "let {};",
                module_binding(&export.local_name, index as u32)
            )?;
        }
        self.write_units(&scan, Some(module.clone()), &mut output)?;
        let classes = scan.uses.get(&main.offset).cloned().unwrap_or_default();
        let body = self.module_body(&plan, Some(module.clone()), &classes)?;
        output.write_all(&body)?;
        self.write_exports(&module, &mut output)?;
        crate::peephole::compact(output.into_bytes())
    }

    pub fn decompile_method(&self, method: &Method) -> Result<Vec<u8>> {
        let scan = self.scan_units(method)?;
        let module = self.module(&self.record(method.record)?)?;
        if scan.uses_module && module.is_none() {
            return Err(invalid("ABC method references missing module metadata"));
        }
        let mut output = Output::new(self.budget.output_bytes);
        if let Some(module) = &module {
            self.write_imports(module, &scan, &mut output)?;
        }
        if scan.class_tracking {
            output.write_all(CLASS_REGISTRY.as_bytes())?;
        }
        if scan.uses_copy_data {
            output.write_all(COPY_HELPER.as_bytes())?;
        }
        self.write_units(&scan, module.clone(), &mut output)?;
        self.render_root(method, &scan, module, &mut output)?;
        crate::peephole::compact(output.into_bytes())
    }

    fn write_units(
        &self,
        scan: &Scan,
        module: Option<Arc<Module>>,
        output: &mut Output,
    ) -> Result<()> {
        for function in &scan.functions {
            let rendered = (|| {
                let plan = self.make_plan(function, true)?;
                let classes = scan.uses.get(&function.offset).cloned().unwrap_or_default();
                write!(
                    output,
                    "function _abc_capture_{:x}(__abc_context) {{\n    const _abc_{:x} = ",
                    function.offset, function.offset
                )?;
                self.render_function_value(
                    &plan,
                    1,
                    output,
                    &FunctionValueOptions {
                        inline: true,
                        module: module.clone(),
                        classes: &classes,
                        top_level_arrow: false,
                    },
                )?;
                write!(output, ";\n    return _abc_{:x};\n}}\n", function.offset)?;
                Ok(())
            })()
            .map_err(|mut error: rxd_core::Error| {
                error.message =
                    format!("{} (nested method:0x{:x})", error.message, function.offset);
                error
            });
            rendered?;
        }
        for class in &scan.classes {
            self.render_class_unit(class, scan, module.clone(), output)
                .map_err(|mut error| {
                    error.message = format!(
                        "{} (nested method:0x{:x})",
                        error.message, class.constructor.offset
                    );
                    error
                })?;
        }
        Ok(())
    }

    fn module_body(
        &self,
        plan: &Plan<'a>,
        module: Option<Arc<Module>>,
        classes: &HashMap<u32, ClassUse>,
    ) -> Result<Vec<u8>> {
        let mut visited = Vec::new();
        visited
            .try_reserve_exact(plan.graph.blocks.len())
            .map_err(|_| resource("ABC region allocation failed"))?;
        visited.resize(plan.graph.blocks.len(), false);
        let mut writer = Writer {
            abc: self,
            decoded: &plan.decoded,
            graph: &plan.graph,
            visited,
            ordinary_receivers: &plan.ordinary_receivers,
            async_plan: plan.async_plan.as_ref(),
            try_regions: &plan.try_regions,
            uses_context: true,
            module,
            implicit: plan.implicit,
            function_ref: None,
            func_unused: plan.func_unused,
            classes,
            super_context: false,
            arrow: plan.arrow,
            top_level_arrow: true,
            module_body: true,
            generator: plan.generator,
            dominance: Vec::new(),
            escape_serial: 0,
            escapes: Vec::new(),
            out: Output::new(self.budget.output_bytes),
        };
        writer.out.write_all(b"let acc")?;
        if plan.uses_env {
            writer.out.write_all(b", __abc_env = __abc_context.env")?;
        }
        if plan.uses_this {
            writer.out.write_all(b", __abc_this = this")?;
        }
        for register in &plan.registers {
            write!(writer.out, ", v{register}")?;
        }
        writer.out.write_all(b";\n")?;
        let mut prefix = Vec::new();
        prefix
            .try_reserve_exact(writer.out.len())
            .map_err(|_| resource("ABC source prefix allocation failed"))?;
        prefix.extend_from_slice(&writer.out);
        let end = writer.graph.blocks.len();
        let structured = writer.range(0, end, end, None, None, 0).and_then(|()| {
            if let Some(index) = writer
                .graph
                .reachable
                .iter()
                .zip(&writer.visited)
                .position(|(reachable, visited)| *reachable && !visited)
            {
                return Err(unsupported(format!(
                    "reachable ABC block {index} was not reconstructed"
                )));
            }
            Ok(())
        });
        if let Err(error) = structured {
            if error.code != "DECOMPILE_FAILED" {
                return Err(error);
            }
            writer.out = Output::new(self.budget.output_bytes);
            writer.out.write_all(&prefix)?;
            writer.visited.fill(false);
            writer.linear(0)?;
        }
        Ok(writer.out.into_bytes())
    }

    fn write_exports(&self, module: &Module, output: &mut Output) -> Result<()> {
        for (index, export) in module.local_exports.iter().enumerate() {
            let binding = module_binding(&export.local_name, index as u32);
            output.write_all(b"export { ")?;
            output.write_all(binding.as_bytes())?;
            output.write_all(b" as ")?;
            export_name(&export.export_name, output)?;
            output.write_all(b" };\n")?;
        }
        for export in &module.indirect_exports {
            let Some(request) = export.request else {
                return Err(invalid("ABC indirect export lacks its module request"));
            };
            let request = module
                .requests
                .get(request as usize)
                .ok_or_else(|| invalid("ABC indirect export request is out of range"))?;
            output.write_all(b"export { ")?;
            export_name(&export.local_name, output)?;
            output.write_all(b" as ")?;
            export_name(&export.export_name, output)?;
            output.write_all(b" } from ")?;
            request.write_quoted(output)?;
            output.write_all(b";\n")?;
        }
        for request in &module.star_exports {
            let request = module
                .requests
                .get(*request as usize)
                .ok_or_else(|| invalid("ABC star export request is out of range"))?;
            output.write_all(b"export * from ")?;
            request.write_quoted(output)?;
            output.write_all(b";\n")?;
        }
        Ok(())
    }

    fn scan_units(&self, root: &Method) -> Result<Scan> {
        let mut scan = Scan::default();
        let mut queue = Vec::new();
        queue
            .try_reserve(1)
            .map_err(|_| resource("ABC definition queue allocation failed"))?;
        queue.push(root.clone());
        let mut scanned = HashSet::new();
        scanned
            .try_reserve(1)
            .map_err(|_| resource("ABC definition set allocation failed"))?;
        let mut seen_functions = HashSet::new();
        seen_functions
            .try_reserve(1)
            .map_err(|_| resource("ABC function set allocation failed"))?;
        let mut seen_classes = HashSet::new();
        seen_classes
            .try_reserve(1)
            .map_err(|_| resource("ABC class set allocation failed"))?;
        while let Some(current) = queue.pop() {
            if !scanned.insert(current.offset) {
                continue;
            }
            let decoded = self.decode(&current)?;
            for instruction in &decoded.instructions {
                match instruction.opcode.name {
                    "ldexternalmodulevar" | "wide.ldexternalmodulevar" => {
                        scan.uses_module = true;
                        scan.imports.push(instruction.unsigned(0)?);
                    }
                    "getmodulenamespace" | "wide.getmodulenamespace" => {
                        scan.uses_module = true;
                        scan.namespaces.push(instruction.unsigned(0)?);
                    }
                    "ldlocalmodulevar"
                    | "wide.ldlocalmodulevar"
                    | "stmodulevar"
                    | "wide.stmodulevar" => scan.uses_module = true,
                    "defineclasswithbuffer" => scan.class_tracking = true,
                    "copydataproperties" => scan.uses_copy_data = true,
                    _ if instruction.opcode.name.starts_with("stown") => scan.class_tracking = true,
                    _ => {}
                }
            }
            let mut requests = Vec::new();
            for instruction in &decoded.instructions {
                if instruction.opcode.name == "defineclasswithbuffer" {
                    requests.push((instruction.pc, instruction.register(4)?));
                }
            }
            let proven = if requests.is_empty() {
                Vec::new()
            } else {
                let graph = Graph::build(&decoded, self.budget)?;
                crate::provenance::proven_hole(self, &decoded, &graph, &requests)?
            };
            let mut uses = HashMap::new();
            let mut class_index = 0usize;
            for instruction in &decoded.instructions {
                match instruction.opcode.name {
                    "definefunc" | "definemethod" => {
                        let target = instruction.entity(1, EntityKind::Method)?;
                        let child = self.method(target)?;
                        if child.record != current.record {
                            return Err(invalid(
                                "ABC function definition belongs to another record",
                            ));
                        }
                        if target != root.offset && seen_functions.insert(target) {
                            self.check_storage::<Method>(scan.functions.len() + 1)?;
                            scan.functions.push(child.clone());
                        }
                        queue.push(child);
                    }
                    "createobjectwithbuffer" | "createarraywithbuffer" => {
                        let offset = instruction.entity(1, EntityKind::Literal)?;
                        for literal in self.literals(offset)?.iter() {
                            let LiteralValue::Method(target) = literal.value else {
                                continue;
                            };
                            let child = self.method(target)?;
                            if child.record != current.record {
                                return Err(invalid(
                                    "ABC literal method belongs to another record",
                                ));
                            }
                            if target != root.offset && seen_functions.insert(target) {
                                self.check_storage::<Method>(scan.functions.len() + 1)?;
                                scan.functions.push(child.clone());
                            }
                            queue.push(child);
                        }
                    }
                    "defineclasswithbuffer" => {
                        let class = ClassPlan::parse(
                            self,
                            &current,
                            instruction,
                            *proven
                                .get(class_index)
                                .ok_or_else(|| invalid("ABC class proof is missing its request"))?,
                        )?;
                        class_index += 1;
                        uses.insert(
                            instruction.pc,
                            ClassUse {
                                constructor: class.constructor.offset,
                                has_base: matches!(
                                    class.base,
                                    crate::class_plan::ClassBase::Register
                                ),
                            },
                        );
                        if seen_classes.insert(class.constructor.offset) {
                            self.check_storage::<ClassPlan>(scan.classes.len() + 1)?;
                            scan.classes.push(class.clone());
                        }
                        queue.push(class.constructor.clone());
                        for member in &class.members {
                            queue.push(member.method.clone());
                        }
                    }
                    _ => {}
                }
            }
            scan.uses.insert(current.offset, uses);
        }
        scan.imports.sort_unstable();
        scan.imports.dedup();
        scan.namespaces.sort_unstable();
        scan.namespaces.dedup();
        Ok(scan)
    }

    fn write_imports(&self, module: &Module, scan: &Scan, output: &mut Output) -> Result<()> {
        for index in &scan.imports {
            let entry = module
                .regular_imports
                .get(*index as usize)
                .or_else(|| {
                    (*index as usize)
                        .checked_sub(module.regular_imports.len())
                        .and_then(|index| module.namespace_imports.get(index))
                })
                .ok_or_else(|| invalid("ABC import slot is outside module metadata"))?;
            let request = module
                .requests
                .get(entry.request as usize)
                .ok_or_else(|| invalid("ABC import module request is out of range"))?;
            match &entry.import_name {
                None => write!(output, "import * as __abc_import_{index} from ")?,
                Some(name) if name.text == "default" => {
                    write!(output, "import __abc_import_{index} from ")?
                }
                Some(name) => {
                    output.write_all(b"import { ")?;
                    name.write_quoted(output)?;
                    write!(output, " as __abc_import_{index} }} from ")?;
                }
            }
            request.write_quoted(output)?;
            output.write_all(b";\n")?;
        }
        for index in &scan.namespaces {
            let request = module
                .requests
                .get(*index as usize)
                .ok_or_else(|| invalid("ABC module namespace request is out of range"))?;
            write!(output, "import * as __abc_namespace_{index} from ")?;
            request.write_quoted(output)?;
            output.write_all(b";\n")?;
        }
        Ok(())
    }

    fn make_plan(&self, method: &Method, force_context: bool) -> Result<Plan<'a>> {
        let decoded = self.decode(method)?;
        let call_type = self.method_call_type(method)?;
        let have_func = call_type & 8 != 0;
        let have_new_target = call_type & 2 != 0;
        let have_this = call_type & 1 != 0;
        let implicit = Implicit {
            args: None,
            params: 0,
            func: have_func.then_some(decoded.code.vregs),
            new_target: have_new_target.then_some(decoded.code.vregs + u32::from(have_func)),
            this_reg: have_this
                .then_some(decoded.code.vregs + u32::from(have_func) + u32::from(have_new_target)),
        };
        let implicit_count =
            u32::from(have_func) + u32::from(have_new_target) + u32::from(have_this);
        let params = decoded
            .code
            .args
            .checked_sub(implicit_count)
            .ok_or_else(|| invalid("ABC call type exceeds the argument frame"))?;
        self.check_storage::<u32>(params as usize)?;
        let implicit = Implicit {
            args: (params > 0).then_some(decoded.code.vregs + implicit_count),
            params,
            ..implicit
        };
        let kind = ((method.flags >> 8) & 0xff) as u8;
        if !matches!(kind, 0 | 1 | 2 | 4 | 6 | 7) {
            return Err(unsupported(format!(
                "ABC function kind {kind:#x} needs additional reconstruction"
            )));
        }
        let arrow = matches!(kind, 2 | 6);
        let mut generator = false;
        let mut generator_register = None;
        let mut generator_save = 0usize;
        for (index, instruction) in decoded.instructions.iter().enumerate() {
            if instruction.opcode.name != "creategeneratorobj" {
                continue;
            }
            if generator {
                return Err(unsupported("method contains several generator objects"));
            }
            let save = decoded
                .instructions
                .get(index + 1)
                .filter(|next| next.opcode.name == "sta")
                .ok_or_else(|| unsupported("generator object is not saved to a register"))?;
            generator = true;
            generator_register = Some(save.register(0)?);
            generator_save = index + 1;
        }
        if let Some(object) = generator_register {
            for (index, instruction) in decoded.instructions.iter().enumerate().skip(generator_save)
            {
                if !instruction.operands.iter().any(
                    |operand| matches!(operand, Operand::Register(register) if *register == object),
                ) {
                    continue;
                }
                match instruction.opcode.name {
                    "creategeneratorobj" => {}
                    "suspendgenerator" | "asyncgeneratorreject"
                        if instruction.register(0)? == object => {}
                    "sta" if instruction.register(0)? == object => {}
                    "resumegenerator" => {}
                    "lda" if instruction.register(0)? == object => {
                        let next = decoded.instructions.get(index + 1);
                        if next.is_none_or(|next| {
                            !matches!(next.opcode.name, "resumegenerator" | "getresumemode")
                        }) {
                            return Err(unsupported("generator object escapes its state machine"));
                        }
                    }
                    _ => {
                        return Err(unsupported(format!(
                            "generator object escapes its state machine at PC {:#x} ({})",
                            instruction.pc, instruction.opcode.name
                        )));
                    }
                }
            }
        }
        if generator {
            validate_generator_resume(&decoded.instructions)?;
        }
        if generator && arrow {
            return Err(unsupported("lexical function cannot be a generator"));
        }
        let mut derived = false;
        let mut uses_super = false;
        for instruction in &decoded.instructions {
            let name = instruction.opcode.name;
            if matches!(
                name,
                "supercallthisrange"
                    | "wide.supercallthisrange"
                    | "supercallarrowrange"
                    | "wide.supercallarrowrange"
                    | "supercallspread"
                    | "callruntime.supercallforwardallargs"
            ) {
                derived = true;
                uses_super = true;
            } else if name.starts_with("ldsuper") || name.starts_with("stsuper") {
                uses_super = true;
            }
            if let Some(range) = crate::instructions::register_range(instruction)? {
                for implicit in [implicit.func, implicit.new_target, implicit.this_reg]
                    .into_iter()
                    .flatten()
                {
                    if range.contains(&u64::from(implicit)) {
                        return Err(unsupported(
                            "implicit argument escapes through an argument range",
                        ));
                    }
                }
            }
        }
        if arrow && derived {
            return Err(unsupported(
                "lexical function performs a super constructor call",
            ));
        }
        let graph = Graph::build(&decoded, self.budget)?;
        let try_regions = simple_tries(&decoded, &graph)?;
        let async_plan = crate::async_plan::simple_async(self, &decoded)?;
        if async_plan.is_some() != matches!(kind, 4 | 6) {
            return Err(unsupported(
                "async function kind and state machine require matching reconstruction",
            ));
        }
        let ordinary_receivers = crate::fresh::ordinary_receivers(self, &decoded, &graph)?;
        let buffer_methods = decoded.instructions.iter().any(|instruction| {
            matches!(
                instruction.opcode.name,
                "createobjectwithbuffer" | "createarraywithbuffer"
            ) && instruction
                .entity(1, EntityKind::Literal)
                .ok()
                .and_then(|offset| self.literals(offset).ok())
                .is_some_and(|literals| {
                    literals
                        .iter()
                        .any(|literal| matches!(literal.value, LiteralValue::Method(_)))
                })
        });
        let uses_env = buffer_methods
            || decoded.instructions.iter().any(|instruction| {
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
                        | "definemethod"
                        | "defineclasswithbuffer"
                )
            });
        let uses_context = force_context
            || uses_env
            || decoded.instructions.iter().any(|instruction| {
                matches!(
                    instruction.opcode.name,
                    "ldhole" | "throw.undefinedifholewithname"
                )
            });
        let uses_this = decoded.instructions.iter().any(|instruction| {
            let name = instruction.opcode.name;
            if name == "ldthis" {
                return true;
            }
            instruction.operands.iter().any(|operand| match operand {
                Operand::Register(register) => Some(*register) == implicit.this_reg,
                _ => false,
            })
        });
        let mut registers = Vec::new();
        for instruction in &decoded.instructions {
            for operand in &instruction.operands {
                if let Operand::Register(register) = operand
                    && *register < decoded.code.vregs
                {
                    registers.push(*register);
                }
            }
            if let Some(range) = crate::instructions::register_range(instruction)? {
                let count = usize::try_from(range.end - range.start)
                    .map_err(|_| resource("ABC register range overflow"))?;
                self.check_storage::<u32>(count)?;
                registers.extend(
                    range
                        .map(|register| register as u32)
                        .filter(|register| *register < decoded.code.vregs),
                );
            }
        }
        registers.sort_unstable();
        registers.dedup();
        let func_unused = crate::provenance::function_value_unused(&decoded)?;
        Ok(Plan {
            method: method.clone(),
            decoded,
            kind,
            arrow,
            derived,
            uses_super,
            params,
            implicit,
            graph,
            try_regions,
            async_plan,
            ordinary_receivers,
            uses_env,
            uses_context,
            uses_this,
            func_unused,
            generator,
            registers,
        })
    }

    fn params(&self, plan: &Plan<'a>) -> String {
        (0..plan.params)
            .map(|index| format!("p{index}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn body(&self, plan: &Plan<'a>, depth: usize, options: &BodyOptions<'_>) -> Result<Vec<u8>> {
        let mut visited = Vec::new();
        visited
            .try_reserve_exact(plan.graph.blocks.len())
            .map_err(|_| resource("ABC region allocation failed"))?;
        visited.resize(plan.graph.blocks.len(), false);
        let mut writer = Writer {
            abc: self,
            decoded: &plan.decoded,
            graph: &plan.graph,
            visited,
            ordinary_receivers: &plan.ordinary_receivers,
            async_plan: plan.async_plan.as_ref(),
            try_regions: &plan.try_regions,
            uses_context: plan.uses_context,
            module: options.module.clone(),
            implicit: plan.implicit,
            function_ref: options.function_ref,
            func_unused: plan.func_unused,
            classes: options.classes,
            super_context: options.super_context,
            arrow: plan.arrow,
            top_level_arrow: options.top_level_arrow,
            module_body: false,
            generator: plan.generator,
            dominance: Vec::new(),
            escape_serial: 0,
            escapes: Vec::new(),
            out: Output::new(self.budget.output_bytes),
        };
        writer.line(depth, "\"use strict\";")?;
        writer.indent(depth)?;
        writer.out.write_all(b"let acc")?;
        if plan.async_plan.as_ref().is_some_and(|plan| plan.awaits) {
            writer.out.write_all(b", __abc_await")?;
        }
        if plan.generator {
            writer.out.write_all(b", __abc_resume")?;
        }
        if plan.uses_env {
            writer.out.write_all(b", __abc_env = __abc_context.env")?;
        }
        if plan.uses_this {
            writer.out.write_all(if plan.derived {
                b", __abc_this = void 0" as &[u8]
            } else {
                b", __abc_this = this"
            })?;
        }
        for register in &plan.registers {
            write!(writer.out, ", v{register}")?;
        }
        writer.out.write_all(b";\n")?;
        let mut prefix = Vec::new();
        prefix
            .try_reserve_exact(writer.out.len())
            .map_err(|_| resource("ABC source prefix allocation failed"))?;
        prefix.extend_from_slice(&writer.out);
        let end = writer.graph.blocks.len();
        let structured = writer.range(0, end, end, None, None, depth).and_then(|()| {
            if let Some(index) = writer
                .graph
                .reachable
                .iter()
                .zip(&writer.visited)
                .position(|(reachable, visited)| *reachable && !visited)
            {
                return Err(unsupported(format!(
                    "reachable ABC block at PC {:#x} of method:0x{:x} was not reconstructed",
                    writer.graph.blocks[index].first, writer.decoded.method
                )));
            }
            Ok(())
        });
        if let Err(error) = structured {
            if error.code != "DECOMPILE_FAILED" {
                return Err(error);
            }
            writer.out = Output::new(self.budget.output_bytes);
            writer.out.write_all(&prefix)?;
            writer.visited.fill(false);
            writer.linear(depth)?;
        }
        Ok(writer.out.into_bytes())
    }

    fn render_function_value(
        &self,
        plan: &Plan<'a>,
        depth: usize,
        output: &mut Output,
        options: &FunctionValueOptions<'_>,
    ) -> Result<()> {
        let name = format!("_abc_{:x}", plan.method.offset);
        let params = self.params(plan);
        let head = match plan.kind {
            2 => format!("({params}) => {{\n"),
            6 => format!("async ({params}) => {{\n"),
            4 => format!("async function {name}({params}) {{\n"),
            _ if plan.generator => format!("function* {name}({params}) {{\n"),
            _ => format!("function {name}({params}) {{\n"),
        };
        if !options.inline {
            indent(output, depth)?;
        }
        output.write_all(head.as_bytes())?;
        let body = self.body(
            plan,
            depth + 1,
            &BodyOptions {
                function_ref: Some(&name),
                super_context: false,
                top_level_arrow: options.top_level_arrow,
                module: options.module.clone(),
                classes: options.classes,
            },
        )?;
        output.write_all(&body)?;
        indent(output, depth)?;
        output.write_all(b"}")?;
        Ok(())
    }

    fn render_class_element(
        &self,
        plan: &Plan<'a>,
        name: &str,
        is_static: bool,
        depth: usize,
        output: &mut Output,
        context: &RenderContext<'_>,
    ) -> Result<()> {
        indent(output, depth)?;
        if is_static {
            output.write_all(b"static ")?;
        }
        if matches!(plan.kind, 4 | 6) {
            output.write_all(b"async ")?;
        }
        if plan.generator {
            output.write_all(b"*")?;
        }
        writeln!(output, "{name}({}) {{", self.params(plan))?;
        let body = self.body(
            plan,
            depth + 1,
            &BodyOptions {
                function_ref: None,
                super_context: true,
                top_level_arrow: false,
                module: context.module.clone(),
                classes: context.classes,
            },
        )?;
        output.write_all(&body)?;
        indent(output, depth)?;
        output.write_all(b"}\n")?;
        Ok(())
    }

    fn render_class_unit(
        &self,
        class: &ClassPlan,
        scan: &Scan,
        module: Option<Arc<Module>>,
        output: &mut Output,
    ) -> Result<()> {
        let has_base = matches!(class.base, crate::class_plan::ClassBase::Register);
        write!(
            output,
            "function _abc_class_{:x}(__abc_context{}",
            class.constructor.offset,
            if has_base { ", __abc_super" } else { "" }
        )?;
        output.write_all(b") {\n    const __abc_class = class")?;
        if let Some(name) = class_name(&class.constructor.name) {
            write!(output, " {name}")?;
        }
        if has_base {
            output.write_all(b" extends __abc_super")?;
        }
        output.write_all(b" {\n")?;
        let empty = HashMap::new();
        let constructor = self.make_plan(&class.constructor, true)?;
        if !matches!(constructor.kind, 0 | 1) {
            return Err(unsupported("class constructor has a lexical function kind"));
        }
        let constructor_context = RenderContext {
            module: module.clone(),
            classes: scan.uses.get(&class.constructor.offset).unwrap_or(&empty),
        };
        self.render_class_element(
            &constructor,
            "constructor",
            false,
            2,
            output,
            &constructor_context,
        )
        .map_err(|mut error| {
            error.message = format!(
                "{} (nested method:0x{:x})",
                error.message, class.constructor.offset
            );
            error
        })?;
        for member in &class.members {
            let rendered = self
                .make_plan(&member.method, true)
                .and_then(|plan| {
                    let context = RenderContext {
                        module: module.clone(),
                        classes: scan.uses.get(&member.method.offset).unwrap_or(&empty),
                    };
                    self.render_class_element(
                        &plan,
                        &element_name(&member.name),
                        member.is_static,
                        2,
                        output,
                        &context,
                    )
                })
                .map_err(|mut error| {
                    error.message = format!(
                        "{} (nested method:0x{:x})",
                        error.message, member.method.offset
                    );
                    error
                });
            rendered?;
        }
        output.write_all(b"    };\n    __abc_class_constructors.add(__abc_class);\n    __abc_class_prototypes.add(__abc_class.prototype);\n    return __abc_class;\n}\n")?;
        Ok(())
    }

    fn render_root(
        &self,
        method: &Method,
        scan: &Scan,
        module: Option<Arc<Module>>,
        output: &mut Output,
    ) -> Result<()> {
        let plan = self.make_plan(method, false)?;
        let classes = scan.uses.get(&method.offset).cloned().unwrap_or_default();
        let name = format!("_abc_{:x}", method.offset);
        if plan.uses_super {
            write!(
                output,
                "function _abc_capture_{:x}(__abc_context, __abc_super) {{\n    return class extends __abc_super {{\n",
                method.offset
            )?;
            if plan.derived {
                let context = RenderContext {
                    module,
                    classes: &classes,
                };
                self.render_class_element(&plan, "constructor", false, 2, output, &context)?;
                output.write_all(b"    };\n}\n")?;
            } else {
                let element = element_name(&method.name);
                let context = RenderContext {
                    module,
                    classes: &classes,
                };
                self.render_class_element(&plan, &element, false, 2, output, &context)?;
                let accessor = if element.starts_with('[') {
                    element.clone()
                } else {
                    format!(".{element}")
                };
                write!(output, "    }}.prototype{accessor};\n}}\n")?;
            }
            return Ok(());
        }
        if plan.uses_context {
            write!(
                output,
                "function _abc_capture_{:x}(__abc_context) {{\n    const {name} = ",
                method.offset
            )?;
            self.render_function_value(
                &plan,
                1,
                output,
                &FunctionValueOptions {
                    inline: true,
                    module,
                    classes: &classes,
                    top_level_arrow: false,
                },
            )?;
            write!(output, ";\n    return {name};\n}}\n")?;
            return Ok(());
        }
        let params = self.params(&plan);
        match plan.kind {
            2 | 6 => {
                writeln!(
                    output,
                    "const {name} = {}({params}) => {{",
                    if plan.kind == 6 { "async " } else { "" }
                )?;
            }
            4 => writeln!(output, "async function {name}({params}) {{")?,
            _ if plan.generator => writeln!(output, "function* {name}({params}) {{")?,
            _ => writeln!(output, "function {name}({params}) {{")?,
        }
        let body = self.body(
            &plan,
            1,
            &BodyOptions {
                function_ref: Some(&name),
                super_context: false,
                top_level_arrow: plan.arrow && !plan.uses_context,
                module,
                classes: &classes,
            },
        )?;
        output.write_all(&body)?;
        output.write_all(if plan.arrow { b"};\n" } else { b"}\n" })?;
        Ok(())
    }
}
