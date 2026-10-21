use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use dotscope::metadata::tables::{
    AssemblyRaw, AssemblyRefRaw, EventMapRaw, EventRaw, FieldRaw, GenericParamConstraintRaw,
    GenericParamRaw, InterfaceImplRaw, MemberRefRaw, MethodDefRaw, MethodSemanticsRaw,
    MethodSpecRaw, NestedClassRaw, PropertyMapRaw, PropertyRaw, TypeDefRaw, TypeRefRaw,
    TypeSpecRaw,
};

use crate::{CallSignature, Field, Input, Method, StringRecord, TypeInfo, input::Work};

pub(crate) fn load(input: &mut Input) -> Result<()> {
    let tables = input.view.tables().context("INVALID_INPUT: tables")?;
    ensure!(
        input.rows[3] == 0
            && input.rows[5] == 0
            && input.rows[7] == 0
            && input.rows[19] == 0
            && input.rows[22] == 0,
        "UNSUPPORTED: pointer metadata tables"
    );
    let mut work = Work::new(input.limits.work);
    reserve_metadata(input, &mut work)?;
    let types = match tables.table::<TypeDefRaw>() {
        Some(t) => t.iter().collect::<dotscope::Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    let fields = match tables.table::<FieldRaw>() {
        Some(t) => t.iter().collect::<dotscope::Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    let methods = match tables.table::<MethodDefRaw>() {
        Some(t) => t.iter().collect::<dotscope::Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    let refs = match tables.table::<MemberRefRaw>() {
        Some(t) => t.iter().collect::<dotscope::Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    let specs = match tables.table::<MethodSpecRaw>() {
        Some(t) => t.iter().collect::<dotscope::Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    let mut names = BTreeMap::new();
    let is_core_library = |name: &str| {
        matches!(
            name,
            "mscorlib" | "System.Private.CoreLib" | "System.Runtime" | "netstandard"
        )
    };
    let own_core_library = if let Some(assembly) = tables
        .table::<AssemblyRaw>()
        .map(|t| t.get(1))
        .transpose()?
        .flatten()
    {
        let name = input.name(assembly.name)?;
        work.charge(name.len())?;
        is_core_library(name)
    } else {
        false
    };
    let mut core_base_types = BTreeSet::new();
    if let Some(refs) = tables.table::<TypeRefRaw>() {
        for row in refs.iter() {
            let row = row?;
            let namespace = input.name(row.type_namespace)?;
            let name = input.name(row.type_name)?;
            work.charge(
                namespace
                    .len()
                    .saturating_add(name.len())
                    .saturating_add(usize::from(!namespace.is_empty())),
            )?;
            let n = full_name(namespace, name);
            if matches!(n.as_str(), "System.ValueType" | "System.Enum") {
                let scope = row.resolution_scope.token.value();
                let core_scope = if scope >> 24 == 35 {
                    let assembly = tables
                        .table::<AssemblyRefRaw>()
                        .context("INVALID_INPUT: assembly reference table")?
                        .get(scope & 0xffffff)?
                        .context("INVALID_INPUT: assembly reference")?;
                    let name = input.name(assembly.name)?;
                    work.charge(name.len())?;
                    is_core_library(name)
                } else {
                    scope >> 24 == 0 && own_core_library
                };
                if core_scope {
                    core_base_types.insert(row.token.value());
                }
            }
            names.insert(row.token.value(), Arc::new(n));
        }
    }
    let mut nested = BTreeMap::new();
    if let Some(rows) = tables.table::<NestedClassRaw>() {
        for row in rows.iter() {
            let row = row?;
            input.check_token(0x02000000 | row.nested_class, &[2])?;
            input.check_token(0x02000000 | row.enclosing_class, &[2])?;
            ensure!(
                nested
                    .insert(row.nested_class, row.enclosing_class)
                    .is_none(),
                "INVALID_INPUT: duplicate nested type"
            );
        }
    }
    let mut generic_parameters = generic_parameters(input, &mut work)?;
    let mut interfaces: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    if let Some(rows) = tables.table::<InterfaceImplRaw>() {
        for row in rows.iter() {
            let row = row?;
            work.charge(std::mem::size_of::<(u32, u32)>())?;
            let owner = 0x02000000 | row.class;
            input.check_token(owner, &[2])?;
            input.check_token(row.interface.token.value(), &[1, 2, 27])?;
            interfaces
                .entry(owner)
                .or_default()
                .push(row.interface.token.value());
        }
    }
    let mut result_types = Vec::new();
    for row in &types {
        let name = input.name(row.type_name)?;
        let namespace = input.name(row.type_namespace)?;
        work.charge(name.len().saturating_add(namespace.len()))?;
        let mut segments = vec![name];
        let mut outer_namespace = namespace;
        let mut rid = row.rid;
        let mut seen = BTreeSet::new();
        while let Some(parent) = nested.get(&rid) {
            ensure!(
                seen.insert(rid) && seen.len() <= 64,
                "INVALID_INPUT: cyclic or excessive nested types"
            );
            let p = types
                .get(
                    (*parent as usize)
                        .checked_sub(1)
                        .context("INVALID_INPUT: nested parent")?,
                )
                .context("INVALID_INPUT: nested parent")?;
            let parent_name = input.name(p.type_name)?;
            work.charge(parent_name.len())?;
            segments.push(parent_name);
            outer_namespace = input.name(p.type_namespace)?;
            rid = *parent;
        }
        segments.reverse();
        let joined_bytes = segments.iter().fold(segments.len() - 1, |bytes, name| {
            bytes.saturating_add(name.len())
        });
        work.charge(
            joined_bytes
                .saturating_add(outer_namespace.len())
                .saturating_add(usize::from(!outer_namespace.is_empty())),
        )?;
        let full = Arc::new(full_name(outer_namespace, &segments.join("+")));
        names.insert(row.token.value(), full.clone());
        if own_core_library && matches!(full.as_str(), "System.ValueType" | "System.Enum") {
            core_base_types.insert(row.token.value());
        }
        result_types.push(TypeInfo {
            id: input.id("type", row.token.value()),
            declaring_type_token: nested.get(&row.rid).map(|rid| 0x02000000 | rid),
            declaring_type_id: nested
                .get(&row.rid)
                .map(|rid| input.id("type", 0x02000000 | rid)),
            extends_token: (row.extends.token.value() & 0xffffff != 0)
                .then_some(row.extends.token.value()),
            kind: "class".to_owned(),
            interfaces: interfaces.remove(&row.token.value()).unwrap_or_default(),
            generic_parameters: generic_parameters
                .remove(&row.token.value())
                .unwrap_or_default(),
            token: row.token.value(),
            name: name.to_owned(),
            namespace: namespace.to_owned(),
            full_name: full,
            flags: row.flags,
        });
    }
    for ty in &mut result_types {
        ty.kind = if ty.flags & 0x20 != 0 {
            "interface"
        } else if ty
            .extends_token
            .is_some_and(|token| core_base_types.contains(&token))
            && !(own_core_library && ty.full_name.as_str() == "System.Enum")
        {
            if ty
                .extends_token
                .and_then(|token| names.get(&token))
                .is_some_and(|name| name.as_str() == "System.Enum")
            {
                "enum"
            } else {
                "struct"
            }
        } else {
            "class"
        }
        .to_owned();
    }
    let mut field_owners = vec![None; fields.len()];
    let mut method_owners = vec![None; methods.len()];
    for (idx, ty) in types.iter().enumerate() {
        let next = types.get(idx + 1);
        assign_range(
            &mut field_owners,
            ty.field_list,
            next.map_or(fields.len() as u32 + 1, |r| r.field_list),
            idx,
        )?;
        assign_range(
            &mut method_owners,
            ty.method_list,
            next.map_or(methods.len() as u32 + 1, |r| r.method_list),
            idx,
        )?;
    }
    ensure!(
        field_owners.iter().all(Option::is_some) && method_owners.iter().all(Option::is_some),
        "INVALID_INPUT: unowned metadata members"
    );
    let mut signatures = Signatures::default();
    let mut result_fields = Vec::new();
    for (idx, row) in fields.iter().enumerate() {
        let signature = signatures.field(input, row.signature, &mut work)?;
        let name = input.name(row.name)?;
        let owner = field_owners[idx].context("INVALID_INPUT: field owner")?;
        let declaring_type = result_types[owner].full_name.clone();
        work.charge(name.len().saturating_add(2))?; // name bytes, shared owner/signature handles
        result_fields.push(Field {
            id: input.id("field", row.token.value()),
            declaring_type_id: result_types[owner].id.clone(),
            token: row.token.value(),
            declaring_type_token: result_types[owner].token,
            declaring_type,
            name: name.to_owned(),
            signature,
            flags: row.flags.try_into()?,
        });
    }
    let mut result_methods = Vec::new();
    let mut member_kinds = BTreeMap::new();
    let type_kinds: BTreeMap<_, _> = types
        .iter()
        .map(|row| {
            let is_value = core_base_types.contains(&row.extends.token.value())
                && !(own_core_library
                    && names
                        .get(&row.token.value())
                        .is_some_and(|name| name.as_str() == "System.Enum"));
            (row.token.value(), is_value)
        })
        .collect();
    for (idx, row) in methods.iter().enumerate() {
        let cached = signatures.method(input, row.signature, &mut work)?;
        let signature = cached.display(&mut work)?;
        let sig = &cached.sig;
        let name = input.name(row.name)?;
        let owner = method_owners[idx].context("INVALID_INPUT: method owner")?;
        let declaring_type = result_types[owner].full_name.clone();
        let token = row.token.value();
        let type_token = types[owner].token.value();
        let is_value_type = type_kinds[&type_token];
        work.charge(name.len().saturating_add(4))?; // name bytes and immutable shared handles
        result_methods.push(Method {
            declaring_type_id: result_types[owner].id.clone(),
            impl_flags: row.impl_flags.try_into()?,
            generic_parameters: generic_parameters
                .remove(&row.token.value())
                .unwrap_or_default(),
            id: input.id("method", token),
            token,
            declaring_type,
            declaring_type_token: type_token,
            declaring_type_is_value_type: is_value_type,
            name: name.to_owned(),
            signature,
            parameters: sig.parameters.clone(),
            return_type: sig.return_type.clone(),
            has_this: sig.has_this,
            signature_flags: sig.flags,
            has_body: row.rva != 0 && row.impl_flags & 3 == 0,
            rva: row.rva,
            flags: row.flags.try_into()?,
        });
    }
    // Validate every reference, but do not build call names/projections until
    // resolve_call asks for one. CLI metadata/FETCH does not consume that index.
    for row in &refs {
        let bytes = input.blob(row.signature)?;
        work.charge(input.name(row.name)?.len())?;
        let is_method = bytes.first() != Some(&6);
        member_kinds.insert(row.token.value(), is_method);
        if is_method {
            signatures.method(input, row.signature, &mut work)?;
        } else {
            signatures.field(input, row.signature, &mut work)?;
        }
    }
    for row in specs {
        let target = row.method.token.value();
        input.check_token(target, &[6, 10])?;
        ensure!(
            target >> 24 != 10 || member_kinds.get(&target) == Some(&true),
            "INVALID_INPUT: MethodSpec references field"
        );
        let args = generic_arguments(input, input.blob(row.instantiation)?, &mut work)?;
        let (parameters, returns, vararg) = if target >> 24 == 6 {
            let method = &result_methods[(target & 0xffffff) as usize - 1];
            (
                &method.parameters,
                &method.return_type,
                matches!(method.signature_flags & 15, 5 | 11),
            )
        } else {
            let row = &refs[(target & 0xffffff) as usize - 1];
            let sig = &signatures.method(input, row.signature, &mut work)?.sig;
            (&sig.parameters, &sig.return_type, sig.vararg)
        };
        if !vararg {
            for ty in parameters.iter().chain(std::iter::once(returns.as_ref())) {
                validate_method_arguments(ty, &args, &mut work)?;
            }
        }
    }
    input.types = result_types;
    input.fields = result_fields;
    input.methods = result_methods;
    input.member_kinds = member_kinds;
    input.type_kinds = type_kinds;
    input.type_names = names;
    load_relationships(input, &mut work)?;
    input.metadata_work = work.used();
    Ok(())
}

// Bound the fixed retained rows/indices before allocating them. Two row slots
// cover Vec growth; full identity strings and conservative tree-node storage are
// included separately. Variable signature/name payloads are bounded by Work.
fn reserve_metadata(input: &Input, work: &mut Work) -> Result<()> {
    let id_bytes = input.source_id.len() + 32;
    let word = std::mem::size_of::<usize>();
    let weights = [
        (
            1,
            12 * (std::mem::size_of::<(u32, Arc<String>)>() + word) + 40,
        ),
        (
            2,
            2 * (std::mem::size_of::<TypeDefRaw>() + std::mem::size_of::<TypeInfo>())
                + 2 * id_bytes
                + 12 * (std::mem::size_of::<(u32, Arc<String>)>() + word)
                + 64,
        ),
        (
            4,
            2 * (std::mem::size_of::<FieldRaw>() + std::mem::size_of::<Field>())
                + 2 * id_bytes
                + std::mem::size_of::<Option<usize>>(),
        ),
        (
            6,
            2 * (std::mem::size_of::<MethodDefRaw>() + std::mem::size_of::<Method>())
                + 2 * id_bytes
                + std::mem::size_of::<Option<usize>>(),
        ),
        (
            10,
            2 * std::mem::size_of::<MemberRefRaw>()
                + 12 * (std::mem::size_of::<(u32, bool)>() + word),
        ),
        (43, 2 * std::mem::size_of::<MethodSpecRaw>()),
        (23, std::mem::size_of::<crate::Property>() + id_bytes),
        (20, std::mem::size_of::<crate::Event>() + id_bytes),
        (24, std::mem::size_of::<crate::Accessor>()),
        (
            42,
            12 * (std::mem::size_of::<(u32, Vec<crate::GenericParameter>)>() + word),
        ),
        (41, 12 * (std::mem::size_of::<(u32, u32)>() + word)),
        (9, 12 * (std::mem::size_of::<(u32, Vec<u32>)>() + word)),
    ];
    for (table, bytes) in weights {
        work.retain(
            (input.rows[table] as usize)
                .checked_mul(bytes)
                .context("RESOURCE_LIMIT: metadata bytes")?,
        )?;
    }
    Ok(())
}

fn generic_parameters(
    input: &Input,
    work: &mut Work,
) -> Result<BTreeMap<u32, Vec<crate::GenericParameter>>> {
    let tables = input.view.tables().context("INVALID_INPUT: tables")?;
    let mut constraints: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    if let Some(rows) = tables.table::<GenericParamConstraintRaw>() {
        for row in rows.iter() {
            let row = row?;
            work.charge(std::mem::size_of::<(u32, u32)>())?;
            input.check_token(0x2a000000 | row.owner, &[42])?;
            input.check_token(row.constraint.token.value(), &[1, 2, 27])?;
            constraints
                .entry(row.owner)
                .or_default()
                .push(row.constraint.token.value());
        }
    }
    let mut result: BTreeMap<u32, Vec<crate::GenericParameter>> = BTreeMap::new();
    if let Some(rows) = tables.table::<GenericParamRaw>() {
        for row in rows.iter() {
            let row = row?;
            let owner = row.owner.token.value();
            input.check_token(owner, &[2, 6])?;
            let name = input.name(row.name)?;
            work.charge(name.len() + std::mem::size_of::<crate::GenericParameter>())?;
            let items = result.entry(owner).or_default();
            ensure!(
                !items.iter().any(|parameter| parameter.number == row.number),
                "INVALID_INPUT: duplicate generic parameter"
            );
            items.push(crate::GenericParameter {
                token: row.token.value(),
                number: row.number,
                flags: row.flags,
                name: name.to_owned(),
                constraints: constraints.remove(&row.rid).unwrap_or_default(),
            });
        }
    }
    for parameters in result.values_mut() {
        parameters.sort_by_key(|parameter| parameter.number);
    }
    Ok(result)
}

fn load_relationships(input: &mut Input, work: &mut Work) -> Result<()> {
    let tables = input.view.tables().context("INVALID_INPUT: tables")?;
    let mut properties = Vec::new();
    let mut events = Vec::new();
    let collection_bytes = (input.rows[23] as usize)
        .checked_mul(std::mem::size_of::<crate::Property>())
        .and_then(|bytes| {
            (input.rows[20] as usize)
                .checked_mul(std::mem::size_of::<crate::Event>())
                .and_then(|events| bytes.checked_add(events))
        })
        .and_then(|bytes| {
            (input.rows[21] as usize)
                .checked_mul(std::mem::size_of::<PropertyMapRaw>())
                .and_then(|maps| bytes.checked_add(maps))
        })
        .and_then(|bytes| {
            (input.rows[18] as usize)
                .checked_mul(std::mem::size_of::<EventMapRaw>())
                .and_then(|maps| bytes.checked_add(maps))
        })
        .context("RESOURCE_LIMIT: relationship collections")?;
    work.charge(collection_bytes)?;
    let maps = tables
        .table::<PropertyMapRaw>()
        .map(|rows| rows.iter().collect::<dotscope::Result<Vec<_>>>())
        .transpose()?
        .unwrap_or_default();
    let mut property_owners = vec![None; input.rows[23] as usize];
    let mut seen = BTreeSet::new();
    for (index, row) in maps.iter().enumerate() {
        work.charge(1)?;
        input.check_token(0x02000000 | row.parent, &[2])?;
        ensure!(
            seen.insert(row.parent),
            "INVALID_INPUT: duplicate property owner"
        );
        assign_range(
            &mut property_owners,
            row.property_list,
            maps.get(index + 1)
                .map_or(input.rows[23] + 1, |next| next.property_list),
            (row.parent - 1) as usize,
        )?;
    }
    if let Some(rows) = tables.table::<PropertyRaw>() {
        for row in rows.iter() {
            let row = row?;
            let owner = input
                .types
                .get(
                    property_owners[row.rid as usize - 1]
                        .context("INVALID_INPUT: unowned property")?,
                )
                .context("INVALID_INPUT: property owner")?;
            let bytes = input.blob(row.signature)?;
            let mut parser = Signature::new(input, bytes, work);
            let flags = parser.byte()?;
            ensure!(
                flags & !0x20 == 8,
                "INVALID_INPUT: property calling convention"
            );
            let count = parser.uint()? as usize;
            ensure!(
                count <= bytes.len(),
                "INVALID_INPUT: property parameter count"
            );
            parser
                .work
                .charge(count.saturating_mul(std::mem::size_of::<String>()))?;
            let return_type = parser.ty(0, false)?;
            let mut parameters = Vec::new();
            for _ in 0..count {
                parameters.push(parser.ty(0, false)?);
            }
            parser.end()?;
            let signature = format!(
                "flags=0x{flags:02x};{return_type}({})",
                parameters.join(",")
            );
            let name = input.name(row.name)?;
            work.charge(name.len() + signature.len() + bytes.len() * 2)?;
            properties.push(crate::Property {
                token: row.token.value(),
                declaring_type_token: owner.token,
                declaring_type_id: owner.id.clone(),
                name: name.to_owned(),
                signature,
                signature_hex: bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
                flags: row.flags,
                accessors: Vec::new(),
            });
        }
    }
    let maps = tables
        .table::<EventMapRaw>()
        .map(|rows| rows.iter().collect::<dotscope::Result<Vec<_>>>())
        .transpose()?
        .unwrap_or_default();
    let mut event_owners = vec![None; input.rows[20] as usize];
    seen.clear();
    for (index, row) in maps.iter().enumerate() {
        work.charge(1)?;
        input.check_token(0x02000000 | row.parent, &[2])?;
        ensure!(
            seen.insert(row.parent),
            "INVALID_INPUT: duplicate event owner"
        );
        assign_range(
            &mut event_owners,
            row.event_list,
            maps.get(index + 1)
                .map_or(input.rows[20] + 1, |next| next.event_list),
            (row.parent - 1) as usize,
        )?;
    }
    if let Some(rows) = tables.table::<EventRaw>() {
        for row in rows.iter() {
            let row = row?;
            let owner = input
                .types
                .get(event_owners[row.rid as usize - 1].context("INVALID_INPUT: unowned event")?)
                .context("INVALID_INPUT: event owner")?;
            input.check_token(row.event_type.token.value(), &[1, 2, 27])?;
            let name = input.name(row.name)?;
            work.charge(name.len() + 1)?;
            events.push(crate::Event {
                token: row.token.value(),
                declaring_type_token: owner.token,
                declaring_type_id: owner.id.clone(),
                name: name.to_owned(),
                event_type_token: row.event_type.token.value(),
                flags: row.flags,
                accessors: Vec::new(),
            });
        }
    }
    if let Some(rows) = tables.table::<MethodSemanticsRaw>() {
        for row in rows.iter() {
            let row = row?;
            work.charge(1)?;
            let token = 0x06000000 | row.method;
            input.check_token(token, &[6])?;
            let method = &input.methods[row.method as usize - 1];
            let association = row.association.token.value();
            input.check_token(association, &[20, 23])?;
            let (owner, accessors) = if association >> 24 == 23 {
                ensure!(
                    matches!(row.semantics, 1 | 2 | 4),
                    "INVALID_INPUT: property semantics"
                );
                let property = &mut properties[(association & 0xffffff) as usize - 1];
                (property.declaring_type_token, &mut property.accessors)
            } else {
                ensure!(
                    matches!(row.semantics, 4 | 8 | 16 | 32),
                    "INVALID_INPUT: event semantics"
                );
                let event = &mut events[(association & 0xffffff) as usize - 1];
                (event.declaring_type_token, &mut event.accessors)
            };
            ensure!(
                method.declaring_type_token == owner,
                "INVALID_INPUT: foreign property or event accessor"
            );
            ensure!(
                !accessors
                    .iter()
                    .any(|accessor| accessor.method_token == token
                        && accessor.semantics == row.semantics),
                "INVALID_INPUT: duplicate accessor association"
            );
            work.charge(std::mem::size_of::<crate::Accessor>() + method.id.len())?;
            accessors.push(crate::Accessor {
                method_token: token,
                method_id: method.id.clone(),
                semantics: row.semantics,
            });
        }
    }
    for accessors in properties
        .iter_mut()
        .map(|property| &mut property.accessors)
        .chain(events.iter_mut().map(|event| &mut event.accessors))
    {
        accessors.sort_by_key(|accessor| (accessor.method_token, accessor.semantics));
    }
    input.properties = properties;
    input.events = events;
    Ok(())
}

fn full_name(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_owned()
    } else {
        format!("{namespace}.{name}")
    }
}

fn assign_range(owners: &mut [Option<usize>], start: u32, end: u32, owner: usize) -> Result<()> {
    ensure!(
        start >= 1 && end >= start && end as usize <= owners.len() + 1,
        "INVALID_INPUT: invalid member ownership range"
    );
    for slot in &mut owners[start as usize - 1..end as usize - 1] {
        ensure!(
            slot.replace(owner).is_none(),
            "INVALID_INPUT: overlapping member ownership"
        );
    }
    Ok(())
}

// A blob offset is meaningful only in this immutable Input. Keep field and method
// grammars separate: a previously validated field must not validate a method row.
#[derive(Default)]
struct Signatures {
    fields: BTreeMap<u32, Arc<String>>,
    methods: BTreeMap<u32, CachedMethodSig>,
}

impl Signatures {
    fn field(&mut self, input: &Input, index: u32, work: &mut Work) -> Result<Arc<String>> {
        work.charge(1)?;
        let signature = match self.fields.entry(index) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                work.charge(2)?;
                work.retain(
                    12 * (std::mem::size_of::<(u32, Arc<String>)>() + std::mem::size_of::<usize>())
                        + std::mem::size_of::<String>()
                        + 2 * std::mem::size_of::<usize>(),
                )?;
                entry.insert(Arc::new(field_signature(input, input.blob(index)?, work)?))
            }
        };
        Ok(signature.clone())
    }

    fn method(
        &mut self,
        input: &Input,
        index: u32,
        work: &mut Work,
    ) -> Result<&mut CachedMethodSig> {
        work.charge(1)?;
        Ok(match self.methods.entry(index) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                work.charge(1)?;
                work.retain(
                    12 * (std::mem::size_of::<(u32, CachedMethodSig)>()
                        + std::mem::size_of::<usize>()),
                )?;
                entry.insert(CachedMethodSig {
                    sig: method_signature(input, input.blob(index)?, work)?,
                    display: None,
                })
            }
        })
    }
}

struct CachedMethodSig {
    sig: MethodSig,
    display: Option<Arc<String>>,
}

impl CachedMethodSig {
    fn display(&mut self, work: &mut Work) -> Result<Arc<String>> {
        if self.display.is_none() {
            let text = self.sig.display();
            work.charge(text.len() + 1)?;
            work.retain(std::mem::size_of::<String>() + 2 * std::mem::size_of::<usize>())?;
            self.display = Some(Arc::new(text));
        }
        Ok(self.display.as_ref().unwrap().clone())
    }
}

pub(crate) struct MethodSig {
    pub(crate) parameters: Arc<Vec<String>>,
    pub(crate) return_type: Arc<String>,
    pub(crate) has_this: bool,
    pub(crate) vararg: bool,
    generic_count: u32,
    sentinel: Option<usize>,
    flags: u8,
}
impl MethodSig {
    fn display(&self) -> String {
        format!(
            "flags=0x{:02x};generic={};sentinel={:?};{}({})",
            self.flags,
            self.generic_count,
            self.sentinel,
            self.return_type,
            self.parameters.join(",")
        )
    }
}

/// Build only the call projection requested by the caller. Full metadata loading
/// already checked all referenced signatures and MethodSpec argument bounds.
pub(crate) fn call_signature(input: &Input, token: u32, work: &mut Work) -> Result<CallSignature> {
    work.charge(1)?;
    input.check_token(token, &[6, 10, 43])?;
    let rid = token & 0xffffff;
    match token >> 24 {
        6 => {
            let method = input
                .methods
                .get(rid as usize - 1)
                .context("INVALID_INPUT: missing method call token")?;
            ensure!(
                !matches!(method.signature_flags & 15, 5 | 11),
                "UNSUPPORTED: vararg call signature"
            );
            work.charge(method.declaring_type.len() + method.name.len() + 2)?;
            Ok(crate::input::method_call(method))
        }
        10 => {
            ensure!(
                input.member_kinds.get(&token) == Some(&true),
                "INVALID_INPUT: call references field"
            );
            let row = input
                .view
                .tables()
                .and_then(|t| t.table::<MemberRefRaw>())
                .context("INVALID_INPUT: MemberRef table")?
                .get(rid)?
                .context("INVALID_INPUT: MemberRef token")?;
            let sig = method_signature(input, input.blob(row.signature)?, work)?;
            ensure!(!sig.vararg, "UNSUPPORTED: vararg call signature");
            let name = input.name(row.name)?;
            let parent = row.class.token.value();
            let owner = input.type_name(parent);
            work.charge(owner.map_or(17, str::len) + name.len() + 2)?;
            let name = match owner {
                Some(owner) => format!("{owner}::{name}"),
                None => format!("token(0x{parent:08x})::{name}"),
            };
            Ok(CallSignature {
                token,
                name,
                parameters: sig.parameters,
                return_type: sig.return_type,
                has_this: sig.has_this,
                signature_flags: sig.flags,
                declaring_type_token: parent,
                method_arguments: None,
            })
        }
        43 => {
            let row = input
                .view
                .tables()
                .and_then(|t| t.table::<MethodSpecRaw>())
                .context("INVALID_INPUT: MethodSpec table")?
                .get(rid)?
                .context("INVALID_INPUT: MethodSpec token")?;
            let target = row.method.token.value();
            input.check_token(target, &[6, 10])?;
            let mut call = call_signature(input, target, work)?;
            let args = generic_arguments(input, input.blob(row.instantiation)?, work)?;
            for ty in call
                .parameters
                .iter()
                .chain(std::iter::once(call.return_type.as_ref()))
            {
                validate_method_arguments(ty, &args, work)?;
            }
            let argument_bytes =
                args.iter().map(String::len).sum::<usize>() + args.len().saturating_sub(1);
            work.charge(
                call.name
                    .len()
                    .saturating_add(argument_bytes.saturating_mul(2))
                    .saturating_add(2),
            )?;
            call.name = format!("{}<{}>", call.name, args.join(","));
            call.token = token;
            call.method_arguments = Some(args);
            Ok(call)
        }
        _ => unreachable!("checked callable token"),
    }
}

pub(crate) fn method_signature(input: &Input, bytes: &[u8], work: &mut Work) -> Result<MethodSig> {
    let mut p = Signature::new(input, bytes, work);
    let sig = p.method(0)?;
    p.end()?;
    Ok(sig)
}
pub(crate) fn field_signature(input: &Input, bytes: &[u8], work: &mut Work) -> Result<String> {
    let mut p = Signature::new(input, bytes, work);
    ensure!(p.byte()? == 6, "INVALID_INPUT: field signature tag");
    let ty = p.ty(0, false)?;
    p.end()?;
    Ok(ty)
}
pub(crate) fn type_signature(input: &Input, bytes: &[u8], work: &mut Work) -> Result<String> {
    let mut p = Signature::new(input, bytes, work);
    let ty = p.ty(0, false)?;
    p.end()?;
    Ok(ty)
}
pub(crate) fn locals(input: &Input, bytes: &[u8], work: &mut Work) -> Result<Vec<String>> {
    let mut p = Signature::new(input, bytes, work);
    ensure!(p.byte()? == 7, "INVALID_INPUT: locals signature tag");
    let count = p.uint()? as usize;
    p.work.charge(
        count
            .checked_mul(std::mem::size_of::<String>())
            .context("RESOURCE_LIMIT: signature collection bytes")?,
    )?;
    ensure!(count <= bytes.len(), "INVALID_INPUT: local count");
    let mut result = Vec::new();
    for _ in 0..count {
        result.push(p.ty(0, false)?);
    }
    p.end()?;
    Ok(result)
}
fn generic_arguments(input: &Input, bytes: &[u8], work: &mut Work) -> Result<Vec<String>> {
    let mut p = Signature::new(input, bytes, work);
    ensure!(p.byte()? == 0x0a, "INVALID_INPUT: MethodSpec signature tag");
    let count = p.uint()? as usize;
    p.work.charge(
        count
            .checked_mul(std::mem::size_of::<String>())
            .context("RESOURCE_LIMIT: signature collection bytes")?,
    )?;
    ensure!(
        count <= bytes.len(),
        "INVALID_INPUT: generic argument count"
    );
    let mut args = Vec::new();
    for _ in 0..count {
        args.push(p.ty(0, false)?);
    }
    p.end()?;
    Ok(args)
}
pub(crate) fn type_arguments(
    input: &Input,
    token: u32,
    work: &mut Work,
) -> Result<Option<Vec<String>>> {
    if token >> 24 != 27 {
        return Ok(None);
    }
    let row = input
        .view
        .tables()
        .and_then(|t| t.table::<TypeSpecRaw>())
        .context("UNSUPPORTED: missing TypeSpec table")?
        .get(token & 0xffffff)?
        .context("UNSUPPORTED: missing owner TypeSpec")?;
    let bytes = input.blob(row.signature)?;
    if bytes.first() != Some(&0x15) {
        return Ok(None);
    }
    let mut p = Signature::new(input, bytes, work);
    p.byte()?;
    ensure!(
        matches!(p.byte()?, 0x11 | 0x12),
        "INVALID_INPUT: generic owner kind"
    );
    p.type_token()?;
    let count = p.uint()? as usize;
    p.work
        .charge(count.saturating_mul(std::mem::size_of::<String>()))?;
    ensure!(count <= bytes.len(), "INVALID_INPUT: generic owner count");
    let mut args = Vec::new();
    for _ in 0..count {
        args.push(p.ty(0, false)?);
    }
    p.end()?;
    Ok(Some(args))
}

fn validate_method_arguments(ty: &str, args: &[String], work: &mut Work) -> Result<()> {
    let mut rest = ty;
    while let Some(index) = rest.find("!!") {
        work.charge(index + 2)?;
        rest = &rest[index + 2..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        ensure!(digits > 0, "INVALID_INPUT: generic signature parameter");
        work.charge(digits)?;
        let n: usize = rest[..digits].parse()?;
        ensure!(n < args.len(), "INVALID_INPUT: MethodSpec argument arity");
        rest = &rest[digits..];
    }
    work.charge(rest.len())?;
    Ok(())
}

// Replace both contexts once. Inserted caller arguments must not be scanned
// again as if they were parameters of the callee or declaring type.
pub(crate) fn instantiate(
    ty: &str,
    type_args: Option<&[String]>,
    method_args: Option<&[String]>,
    same_owner: bool,
    work: &mut Work,
) -> Result<String> {
    let mut out = String::new();
    let mut rest = ty;
    while let Some(index) = rest.find('!') {
        work.charge(index)?;
        out.push_str(&rest[..index]);
        rest = &rest[index + 1..];
        let method_parameter = rest.starts_with('!');
        if method_parameter {
            rest = &rest[1..];
        }
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        ensure!(digits > 0, "INVALID_INPUT: generic signature parameter");
        let n: usize = rest[..digits].parse()?;
        let args = if method_parameter {
            method_args
        } else {
            type_args
        };
        if let Some(args) = args {
            let argument = args
                .get(n)
                .context("INVALID_INPUT: generic argument arity")?;
            work.charge(argument.len())?;
            out.push_str(argument);
        } else {
            ensure!(
                !method_parameter && same_owner,
                "UNSUPPORTED: unresolved member generic context"
            );
            work.charge(digits + 1)?;
            out.push('!');
            out.push_str(&rest[..digits]);
        }
        rest = &rest[digits..];
    }
    work.charge(rest.len())?;
    out.push_str(rest);
    Ok(out)
}

struct Signature<'a, 'w> {
    input: &'a Input,
    bytes: &'a [u8],
    pos: usize,
    work: &'w mut Work,
}
impl<'a, 'w> Signature<'a, 'w> {
    fn new(input: &'a Input, bytes: &'a [u8], work: &'w mut Work) -> Self {
        Self {
            input,
            bytes,
            pos: 0,
            work,
        }
    }
    fn byte(&mut self) -> Result<u8> {
        self.work.charge(1)?;
        let b = *self
            .bytes
            .get(self.pos)
            .context("INVALID_INPUT: truncated signature")?;
        self.pos += 1;
        Ok(b)
    }
    fn uint(&mut self) -> Result<u32> {
        compressed(self.bytes, &mut self.pos, self.work).map(|(v, _)| v)
    }
    fn sint(&mut self) -> Result<i32> {
        let (value, width) = compressed(self.bytes, &mut self.pos, self.work)?;
        let mut result = (value >> 1) as i32;
        if value & 1 != 0 {
            result |= match width {
                1 => !0x3f,
                2 => !0x1fff,
                _ => !0x0fff_ffff,
            };
        }
        Ok(result)
    }
    fn end(&self) -> Result<()> {
        ensure!(
            self.pos == self.bytes.len(),
            "INVALID_INPUT: trailing signature bytes"
        );
        Ok(())
    }
    fn type_token(&mut self) -> Result<u32> {
        let coded = self.uint()?;
        let table = match coded & 3 {
            0 => 2,
            1 => 1,
            2 => 27,
            _ => bail!("INVALID_INPUT: signature type token tag"),
        };
        let token = (table << 24) | (coded >> 2);
        self.input.check_token(token, &[1, 2, 27])?;
        Ok(token)
    }
    fn method(&mut self, depth: usize) -> Result<MethodSig> {
        ensure!(depth <= 64, "RESOURCE_LIMIT: signature nesting");
        let flags = self.byte()?;
        ensure!(
            flags & 0x80 == 0
                && matches!(flags & 15, 0..=5 | 9 | 11)
                && (flags & 0x40 == 0 || flags & 0x20 != 0),
            "INVALID_INPUT: calling convention flags 0x{flags:02x}"
        );
        let generic_count = if flags & 0x10 != 0 { self.uint()? } else { 0 };
        let count = self.uint()? as usize;
        self.work.charge(
            count
                .checked_mul(std::mem::size_of::<String>())
                .context("RESOURCE_LIMIT: signature collection bytes")?,
        )?;
        ensure!(count <= self.bytes.len(), "INVALID_INPUT: parameter count");
        let return_type = self.ty(depth + 1, true)?;
        let mut parameters = Vec::new();
        let mut sentinel = None;
        for _ in 0..count {
            if self.bytes.get(self.pos) == Some(&0x41) {
                ensure!(
                    matches!(flags & 15, 5 | 11) && sentinel.is_none(),
                    "INVALID_INPUT: signature sentinel"
                );
                self.byte()?;
                sentinel = Some(parameters.len());
            }
            parameters.push(self.ty(depth + 1, false)?);
        }
        // Shared immutable signature storage; the allocation is charged once.
        self.work.charge(2)?;
        self.work
            .retain(2 * (std::mem::size_of::<String>() + 2 * std::mem::size_of::<usize>()))?;
        Ok(MethodSig {
            parameters: Arc::new(parameters),
            return_type: Arc::new(return_type),
            has_this: flags & 0x20 != 0,
            vararg: matches!(flags & 15, 5 | 11),
            generic_count,
            sentinel,
            flags,
        })
    }
    fn ty(&mut self, depth: usize, allow_void: bool) -> Result<String> {
        ensure!(depth <= 64, "RESOURCE_LIMIT: signature nesting");
        let tag = self.byte()?;
        let primitive = match tag {
            1 if allow_void => Some("void"),
            2 => Some("bool"),
            3 => Some("char"),
            4 => Some("int8"),
            5 => Some("uint8"),
            6 => Some("int16"),
            7 => Some("uint16"),
            8 => Some("int32"),
            9 => Some("uint32"),
            10 => Some("int64"),
            11 => Some("uint64"),
            12 => Some("float32"),
            13 => Some("float64"),
            14 => Some("string"),
            0x16 => Some("typedref"),
            0x18 => Some("native int"),
            0x19 => Some("native uint"),
            0x1c => Some("object"),
            _ => None,
        };
        if let Some(t) = primitive {
            self.work.charge(t.len())?;
            return Ok(t.to_string());
        }
        let result = match tag {
            0x0f => format!("{}*", self.ty(depth + 1, true)?),
            0x10 => format!("{}&", self.ty(depth + 1, false)?),
            0x11 | 0x12 => {
                let token = self.type_token()?;
                format!(
                    "{}(0x{token:08x})",
                    if tag == 0x11 { "valuetype" } else { "class" }
                )
            }
            0x13 => format!("!{}", self.uint()?),
            0x1e => format!("!!{}", self.uint()?),
            0x1d => format!("{}[]", self.ty(depth + 1, false)?),
            0x15 => {
                let kind = self.byte()?;
                ensure!(
                    matches!(kind, 0x11 | 0x12),
                    "INVALID_INPUT: generic type kind"
                );
                let token = self.type_token()?;
                let count = self.uint()? as usize;
                self.work.charge(
                    count
                        .checked_mul(std::mem::size_of::<String>())
                        .context("RESOURCE_LIMIT: signature collection bytes")?,
                )?;
                ensure!(count <= self.bytes.len(), "INVALID_INPUT: generic count");
                let mut args = Vec::new();
                for _ in 0..count {
                    args.push(self.ty(depth + 1, false)?);
                }
                format!(
                    "genericinst({}(0x{token:08x})<{}>)",
                    if kind == 0x11 { "valuetype" } else { "class" },
                    args.join(",")
                )
            }
            0x14 => {
                let elem = self.ty(depth + 1, false)?;
                let rank = self.uint()?;
                let ns = self.uint()?;
                ensure!(rank > 0 && ns <= rank, "INVALID_INPUT: array shape");
                self.work.charge(
                    (ns as usize)
                        .checked_mul(std::mem::size_of::<u32>())
                        .context("RESOURCE_LIMIT: array shape bytes")?,
                )?;
                let mut sizes = Vec::new();
                for _ in 0..ns {
                    sizes.push(self.uint()?);
                }
                let nb = self.uint()?;
                ensure!(nb <= rank, "INVALID_INPUT: array lower bounds");
                self.work.charge(
                    (nb as usize)
                        .checked_mul(std::mem::size_of::<i32>())
                        .context("RESOURCE_LIMIT: array shape bytes")?,
                )?;
                let mut bounds = Vec::new();
                for _ in 0..nb {
                    bounds.push(self.sint()?);
                }
                format!("array({elem};rank={rank};sizes={sizes:?};bounds={bounds:?})")
            }
            0x1f | 0x20 => {
                let token = self.type_token()?;
                let inner = self.ty(depth + 1, allow_void)?;
                format!(
                    "{}(0x{token:08x}) {inner}",
                    if tag == 0x1f { "modreq" } else { "modopt" }
                )
            }
            0x45 => format!("pinned {}", self.ty(depth + 1, false)?),
            0x1b => {
                let sig = self.method(depth + 1)?;
                format!("fnptr({})", sig.display())
            }
            _ => bail!("INVALID_INPUT: unsupported signature element 0x{tag:02x}"),
        };
        self.work.charge(result.len())?;
        Ok(result)
    }
}

pub(crate) fn compressed(bytes: &[u8], pos: &mut usize, work: &mut Work) -> Result<(u32, usize)> {
    let first = *bytes
        .get(*pos)
        .context("INVALID_INPUT: compressed integer")?;
    let width = if first & 0x80 == 0 {
        1
    } else if first & 0xc0 == 0x80 {
        2
    } else if first & 0xe0 == 0xc0 {
        4
    } else {
        bail!("INVALID_INPUT: compressed integer prefix")
    };
    let data = bytes
        .get(
            *pos..pos
                .checked_add(width)
                .context("INVALID_INPUT: compressed offset")?,
        )
        .context("INVALID_INPUT: compressed integer truncated")?;
    work.charge(width)?;
    *pos += width;
    let value = match width {
        1 => first as u32,
        2 => ((first as u32 & 0x3f) << 8) | data[1] as u32,
        _ => {
            ((first as u32 & 0x1f) << 24)
                | ((data[1] as u32) << 16)
                | ((data[2] as u32) << 8)
                | data[3] as u32
        }
    };
    Ok((value, width))
}

pub(crate) fn strings(input: &Input) -> Result<Vec<StringRecord>> {
    strings_with_work(input, &mut Work::new(input.limits.work))
}

pub(crate) fn strings_with_work(input: &Input, work: &mut Work) -> Result<Vec<StringRecord>> {
    let Some(heap) = input.view.userstrings() else {
        return Ok(Vec::new());
    };
    let bytes = heap.data();
    ensure!(
        bytes.first().is_none_or(|b| *b == 0),
        "INVALID_INPUT: user string heap prefix"
    );
    let mut pos = 1usize;
    let mut result = Vec::new();
    let mut output = 0usize;
    while pos < bytes.len() {
        let offset = pos;
        let (length, _) = compressed(bytes, &mut pos, work)?;
        if length == 0 {
            continue;
        }
        let length = length as usize;
        ensure!(length % 2 == 1, "INVALID_INPUT: user string byte length");
        let data = bytes
            .get(
                pos..pos
                    .checked_add(length)
                    .context("INVALID_INPUT: user string overflow")?,
            )
            .context("INVALID_INPUT: truncated user string")?;
        work.charge(length)?;
        output = output
            .checked_add(length.checked_mul(3).context("RESOURCE_LIMIT: strings")?)
            .context("RESOURCE_LIMIT: strings")?;
        ensure!(
            data[length - 1] <= 1,
            "INVALID_INPUT: user string terminal flag"
        );
        let units: Vec<u16> = data[..length - 1]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_le_bytes([v[0], v[1]]))
            .collect();
        let (value, utf16_units) = match String::from_utf16(&units) {
            Ok(s) => (Some(s), None),
            Err(_) => (None, Some(units)),
        };
        result.push(StringRecord {
            offset: offset.try_into()?,
            value,
            utf16_units,
        });
        pos += length;
    }
    Ok(result)
}
