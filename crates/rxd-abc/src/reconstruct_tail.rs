#[derive(Default)]
struct Scan {
    functions: Vec<Method>,
    classes: Vec<ClassPlan>,
    uses: HashMap<u32, HashMap<u32, ClassUse>>,
    imports: Vec<u32>,
    namespaces: Vec<u32>,
    uses_module: bool,
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
        let scan = self.scan_units(method)?;
        let module = self.module(&self.record(method.record)?)?;
        if scan.uses_module && module.is_none() {
            return Err(invalid("ABC method references missing module metadata"));
        }
        let mut output = Output::new(self.budget.output_bytes);
        if let Some(module) = &module {
            self.write_imports(module, &scan, &mut output)?;
        }
        for function in &scan.functions {
            let plan = self.make_plan(function, true)?;
            let classes = scan.uses.get(&function.offset).cloned().unwrap_or_default();
            write!(
                output,
                "function _abc_capture_{:x}(__abc_context) {{\n    const _abc_{:x} = ",
                function.offset, function.offset
            )?;
            self.render_function_value(&plan, 1, &mut output, module.clone(), &classes)?;
            write!(output, ";\n    return _abc_{:x};\n}}\n", function.offset)?;
        }
        for class in &scan.classes {
            self.render_class_unit(class, &scan, module.clone(), &mut output)?;
        }
        self.render_root(method, &scan, module, &mut output)?;
        Ok(output.into_bytes())
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
                    _ => {}
                }
            }
            let graph = Graph::build(&decoded, self.budget)?;
            let mut requests = Vec::new();
            for instruction in &decoded.instructions {
                if instruction.opcode.name == "defineclasswithbuffer" {
                    requests.push((instruction.pc, instruction.register(4)?));
                }
            }
            let proven = crate::provenance::proven_hole(self, &decoded, &graph, &requests)?;
            let mut uses = HashMap::new();
            for (index, instruction) in decoded.instructions.iter().enumerate() {
                match instruction.opcode.name {
                    "definefunc" | "definemethod" => {
                        let target = instruction.entity(1, EntityKind::Method)?;
                        let child = self.method(target)?;
                        if child.record != current.record {
                            return Err(invalid(
                                "ABC function definition belongs to another record",
                            ));
                        }
                        if target != root.offset
                            && !scan.functions.iter().any(|item| item.offset == target)
                        {
                            self.check_storage::<Method>(scan.functions.len() + 1)?;
                            scan.functions.push(child.clone());
                        }
                        queue.push(child);
                    }
                    "defineclasswithbuffer" => {
                        let class = ClassPlan::parse(self, &current, instruction, proven[index])?;
                        uses.insert(
                            instruction.pc,
                            ClassUse {
                                constructor: class.constructor.offset,
                                has_base: matches!(class.base, crate::class_plan::ClassBase::Register(_)),
                            },
                        );
                        if !scan
                            .classes
                            .iter()
                            .any(|item| item.constructor.offset == class.constructor.offset)
                        {
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
            func: have_func.then_some(decoded.code.vregs),
            new_target: have_new_target.then_some(decoded.code.vregs + u32::from(have_func)),
            this_reg: have_this.then_some(
                decoded.code.vregs + u32::from(have_func) + u32::from(have_new_target),
            ),
        };
        let implicit_count =
            u32::from(have_func) + u32::from(have_new_target) + u32::from(have_this);
        let params = decoded
            .code
            .args
            .checked_sub(implicit_count)
            .ok_or_else(|| invalid("ABC call type exceeds the argument frame"))?;
        self.check_storage::<u32>(params as usize)?;
        let kind = (method.flags >> 8) & 0xff;
        if !matches!(kind, 0 | 1 | 2 | 4 | 6) {
            return Err(unsupported(format!(
                "ABC function kind {kind:#x} needs additional reconstruction"
            )));
        }
        let arrow = matches!(kind, 2 | 6);
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
                for implicit in [
                    implicit.func,
                    implicit.new_target,
                    implicit.this_reg,
                ]
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
                    | "definemethod"
                    | "defineclasswithbuffer"
            )
        });
        let uses_context = force_context
            || uses_env
            || decoded
                .instructions
                .iter()
                .any(|instruction| matches!(
                    instruction.opcode.name,
                    "ldhole" | "throw.undefinedifholewithname"
                ));
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
            registers,
        })
    }

    fn params(&self, plan: &Plan<'a>) -> String {
        (0..plan.params)
            .map(|index| format!("p{index}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn body(
        &self,
        plan: &Plan<'a>,
        depth: usize,
        function_ref: Option<&str>,
        super_context: bool,
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
            in_try: false,
            uses_context: plan.uses_context,
            module,
            implicit: plan.implicit,
            derived: plan.derived,
            function_ref,
            classes,
            super_context,
            out: Output::new(self.budget.output_bytes),
        };
        writer.line(depth, "\"use strict\";")?;
        writer.indent(depth)?;
        writer.out.write_all(b"let acc")?;
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
        Ok(writer.out.into_bytes())
    }

    fn render_function_value(
        &self,
        plan: &Plan<'a>,
        depth: usize,
        output: &mut Output,
        module: Option<Arc<Module>>,
        classes: &HashMap<u32, ClassUse>,
    ) -> Result<()> {
        let name = format!("_abc_{:x}", plan.method.offset);
        let params = self.params(plan);
        let head = match plan.kind {
            2 => format!("({params}) => {{\n"),
            6 => format!("async ({params}) => {{\n"),
            4 => format!("async function {name}({params}) {{\n"),
            _ => format!("function {name}({params}) {{\n"),
        };
        indent(output, depth)?;
        output.write_all(head.as_bytes())?;
        let body = self.body(
            plan,
            depth + 1,
            Some(&name),
            false,
            module,
            classes,
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
        module: Option<Arc<Module>>,
        classes: &HashMap<u32, ClassUse>,
    ) -> Result<()> {
        indent(output, depth)?;
        if is_static {
            output.write_all(b"static ")?;
        }
        if matches!(plan.kind, 4 | 6) {
            output.write_all(b"async ")?;
        }
        write!(output, "{name}({}) {{\n", self.params(plan))?;
        let body = self.body(plan, depth + 1, None, true, module, classes)?;
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
        let has_base = matches!(class.base, crate::class_plan::ClassBase::Register(_));
        write!(
            output,
            "function _abc_class_{:x}(__abc_context{}",
            class.constructor.offset,
            if has_base { ", __abc_super" } else { "" }
        )?;
        output.write_all(b") {\n    return class")?;
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
        self.render_class_element(
            &constructor,
            "constructor",
            false,
            2,
            output,
            module.clone(),
            scan.uses.get(&class.constructor.offset).unwrap_or(&empty),
        )?;
        for member in &class.members {
            let plan = self.make_plan(&member.method, true)?;
            self.render_class_element(
                &plan,
                &element_name(&member.name),
                member.is_static,
                2,
                output,
                module.clone(),
                scan.uses.get(&member.method.offset).unwrap_or(&empty),
            )?;
        }
        output.write_all(b"    };\n}\n")?;
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
        if plan.uses_super {
            write!(
                output,
                "function _abc_capture_{:x}(__abc_context, __abc_super) {{\n    return class extends __abc_super {{\n",
                method.offset
            )?;
            if plan.derived {
                self.render_class_element(
                    &plan,
                    "constructor",
                    false,
                    2,
                    output,
                    module,
                    &classes,
                )?;
                output.write_all(b"    };\n}\n")?;
            } else {
                let name = element_name(&method.name);
                self.render_class_element(
                    &plan,
                    &name,
                    false,
                    2,
                    output,
                    module,
                    &classes,
                )?;
                let accessor = if name.starts_with('[') {
                    name.clone()
                } else {
                    format!(".{name}")
                };
                write!(output, "    }}.prototype{accessor};\n}}\n")?;
            }
            return Ok(());
        }
        let name = format!("_abc_{:x}", method.offset);
        let params = self.params(&plan);
        match plan.kind {
            2 | 6 => {
                write!(
                    output,
                    "const {name} = {}({params}) => {{\n",
                    if plan.kind == 6 { "async " } else { "" }
                )?;
            }
            4 => write!(output, "async function {name}({params}) {{\n")?,
            _ => write!(output, "function {name}({params}) {{\n")?,
        }
        let body = self.body(
            &plan,
            1,
            Some(&name),
            false,
            module,
            &classes,
        )?;
        output.write_all(&body)?;
        output.write_all(if plan.arrow { b"};\n" } else { b"}\n" })?;
        Ok(())
    }
}
