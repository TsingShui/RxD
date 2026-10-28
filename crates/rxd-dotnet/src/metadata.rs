use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use cildec::TableId;
use cildec::tables::{
    EventMapRow, EventRow, FieldRow, GenericParamConstraintRow, GenericParamRow, InterfaceImplRow,
    MemberRefRow, MethodDefRow, MethodSemanticsRow, MethodSpecRow, NestedClassRow, PropertyMapRow,
    PropertyRow, TableRow, TypeDefRow, TypeRefRow,
};

use crate::{
    CallSignature, Field, Input, Method, StringRecord, TypeInfo,
    input::{Work, token_of},
};

fn empty_signature() -> std::sync::Arc<String> {
    static EMPTY: std::sync::OnceLock<std::sync::Arc<String>> = std::sync::OnceLock::new();
    EMPTY
        .get_or_init(|| std::sync::Arc::new(String::new()))
        .clone()
}

fn empty_parameters() -> std::sync::Arc<Vec<String>> {
    static EMPTY: std::sync::OnceLock<std::sync::Arc<Vec<String>>> = std::sync::OnceLock::new();
    EMPTY
        .get_or_init(|| std::sync::Arc::new(Vec::new()))
        .clone()
}

fn rows<R: TableRow>(tables: &cildec::Tables<'_>) -> Result<Vec<R>> {
    tables
        .iter::<R>()
        .map(|(_, row)| row)
        .collect::<std::result::Result<Vec<R>, cildec::Error>>()
        .map_err(anyhow::Error::from)
}

pub(crate) fn load(input: &mut Input<'_>) -> Result<()> {
    let tables = input.tables();
    ensure!(
        input.rows[3] == 0
            && input.rows[5] == 0
            && input.rows[7] == 0
            && input.rows[19] == 0
            && input.rows[22] == 0,
        "UNSUPPORTED: pointer metadata tables"
    );
    let mut work = Work::new(input.limits.work, input.limits.metadata_bytes);
    // A row that cannot be indexed is recorded and skipped; other rows still answer.
    let mut unreadable: Vec<crate::Unreadable> = Vec::new();
    let mut skipped_types: BTreeSet<u32> = BTreeSet::new();
    macro_rules! unreadable {
        ($kind:expr, $token:expr, $error:expr) => {{
            let error = $error;
            let reason = format!("{error:#}");
            // Budget exhaustion is not a bad row: it must still fail the read.
            if reason.contains("RESOURCE_LIMIT") {
                return Err(error.into());
            }
            unreadable.push(crate::Unreadable {
                token: $token,
                kind: $kind,
                reason,
            })
        }};
    }
    reserve_metadata(input, &mut work)?;
    let types = rows::<TypeDefRow>(tables)?;
    let fields = rows::<FieldRow>(tables)?;
    let methods = rows::<MethodDefRow>(tables)?;
    let mut names = BTreeMap::new();
    let is_core_library = |name: &str| {
        matches!(
            name,
            "mscorlib" | "System.Private.CoreLib" | "System.Runtime" | "netstandard"
        )
    };
    let own_core_library = if tables.row_count(TableId::Assembly) >= 1 {
        let assembly = tables.assembly(1).context("INVALID_INPUT: assembly row")?;
        let name = input.name(assembly.name)?;
        work.charge(name.len())?;
        is_core_library(name)
    } else {
        false
    };
    let mut core_base_types = BTreeSet::new();
    for (rid, row) in tables.iter::<TypeRefRow>() {
        let token = token_of(TableId::TypeRef, rid);
        let row = match row {
            Ok(row) => row,
            Err(error) => {
                unreadable!("type_ref", None, error);
                continue;
            }
        };
        let processed = (|| -> Result<()> {
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
                let scope = row.resolution_scope.value();
                let core_scope = if scope >> 24 == 35 {
                    let assembly = tables
                        .assembly_ref(scope & 0xffffff)
                        .context("INVALID_INPUT: assembly reference")?;
                    let name = input.name(assembly.name)?;
                    work.charge(name.len())?;
                    is_core_library(name)
                } else {
                    scope >> 24 == 0 && own_core_library
                };
                if core_scope {
                    core_base_types.insert(token);
                }
            }
            names.insert(token, Arc::new(n));
            Ok(())
        })();
        if let Err(error) = processed {
            unreadable!("type_ref", Some(token), error);
        }
    }
    let mut nested = BTreeMap::new();
    for (_, row) in tables.iter::<NestedClassRow>() {
        let row = match row {
            Ok(row) => row,
            Err(error) => {
                unreadable!("nested_class", None, error);
                continue;
            }
        };
        let processed = (|| -> Result<()> {
            input.check_token(0x02000000 | row.nested_class.get(), &[2])?;
            input.check_token(0x02000000 | row.enclosing_class.get(), &[2])?;
            ensure!(
                nested
                    .insert(row.nested_class.get(), row.enclosing_class.get())
                    .is_none(),
                "INVALID_INPUT: duplicate nested type"
            );
            Ok(())
        })();
        if let Err(error) = processed {
            let token = 0x02000000 | row.nested_class.get();
            skipped_types.insert(token);
            unreadable!("nested_class", Some(token), error);
        }
    }
    let mut generic_parameters = generic_parameters(input, &mut work, &mut unreadable)?;
    let mut interfaces: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (_, row) in tables.iter::<InterfaceImplRow>() {
        let row = match row {
            Ok(row) => row,
            Err(error) => {
                unreadable!("interface_impl", None, error);
                continue;
            }
        };
        let owner = 0x02000000 | row.class.get();
        let processed = (|| -> Result<()> {
            work.charge(std::mem::size_of::<(u32, u32)>())?;
            input.check_token(owner, &[2])?;
            input.check_token(row.interface.value(), &[1, 2, 27])?;
            interfaces
                .entry(owner)
                .or_default()
                .push(row.interface.value());
            Ok(())
        })();
        if let Err(error) = processed {
            unreadable!("interface_impl", Some(owner), error);
        }
    }
    let mut result_types = Vec::new();
    // Original TypeDef row index -> position in `result_types`, which can be shorter
    // when a row was unreadable.
    let mut type_slot: Vec<Option<usize>> = vec![None; types.len()];
    for (row_index, row) in types.iter().enumerate() {
        let type_rid = row_index as u32 + 1;
        let token = 0x02000000 | type_rid;
        if skipped_types.contains(&token) {
            continue;
        }
        let processed = (|| -> Result<TypeInfo> {
            let name = input.name(row.type_name)?;
            let namespace = input.name(row.type_namespace)?;
            work.charge(name.len().saturating_add(namespace.len()))?;
            let mut segments = vec![name];
            let mut outer_namespace = namespace;
            let mut rid = type_rid;
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
            names.insert(token, full.clone());
            if own_core_library && matches!(full.as_str(), "System.ValueType" | "System.Enum") {
                core_base_types.insert(token);
            }
            Ok(TypeInfo {
                declaring_type_token: nested.get(&type_rid).map(|rid| 0x02000000 | rid),
                extends_token: (row.extends.value() & 0xffffff != 0).then_some(row.extends.value()),
                kind: "class".to_owned(),
                interfaces: interfaces.remove(&token).unwrap_or_default(),
                generic_parameters: generic_parameters.remove(&token).unwrap_or_default(),
                token,
                name: name.to_owned(),
                namespace: namespace.to_owned(),
                full_name: full,
                flags: row.flags,
            })
        })();
        match processed {
            Ok(ty) => {
                type_slot[row_index] = Some(result_types.len());
                result_types.push(ty);
            }
            Err(error) => {
                skipped_types.insert(token);
                unreadable!("type", Some(token), error);
            }
        }
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
        let assigned = (|| -> Result<()> {
            assign_range(
                &mut field_owners,
                ty.field_list.get(),
                next.map_or(fields.len() as u32 + 1, |r| r.field_list.get()),
                idx,
            )?;
            assign_range(
                &mut method_owners,
                ty.method_list.get(),
                next.map_or(methods.len() as u32 + 1, |r| r.method_list.get()),
                idx,
            )?;
            Ok(())
        })();
        if let Err(error) = assigned {
            let token = 0x02000000 | (idx as u32 + 1);
            skipped_types.insert(token);
            unreadable!("type_members", Some(token), error);
        }
    }
    // Resolve signatures/relationships lazily; unrelated corrupt rows cannot fail local queries.
    let mut result_fields = Vec::new();
    for (idx, row) in fields.iter().enumerate() {
        let token = 0x04000000 | (idx as u32 + 1);
        let processed = (|| -> Result<Field> {
            let name = input.name(row.name)?;
            let owner = field_owners[idx].context("INVALID_INPUT: field owner")?;
            let slot = type_slot[owner].context("INVALID_INPUT: declaring type is unreadable")?;
            let declaring_type = result_types[slot].full_name.clone();
            work.charge(name.len().saturating_add(2))?; // name bytes, shared owner handle
            Ok(Field {
                token,
                declaring_type_token: result_types[slot].token,
                declaring_type,
                name: name.to_owned(),
                signature: empty_signature(),
                signature_index: row.signature.get(),
                resolved: false,
                flags: row.flags,
            })
        })();
        match processed {
            Ok(field) => result_fields.push(field),
            Err(error) => unreadable!("field", Some(token), error),
        }
    }
    let mut result_methods = Vec::new();
    let type_kinds: BTreeMap<_, _> = types
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let token = 0x02000000 | (index as u32 + 1);
            let is_value = core_base_types.contains(&row.extends.value())
                && !(own_core_library
                    && names
                        .get(&token)
                        .is_some_and(|name| name.as_str() == "System.Enum"));
            (token, is_value)
        })
        .collect();
    // Value-type flag per result_types slot, so the method loop does not probe a
    // map once per row.
    let value_slots: Vec<bool> = result_types
        .iter()
        .map(|ty| type_kinds.get(&ty.token).copied().unwrap_or(false))
        .collect();
    for (idx, row) in methods.iter().enumerate() {
        let processed = (|| -> Result<Method> {
            let name = input.name(row.name)?;
            let owner = method_owners[idx].context("INVALID_INPUT: method owner")?;
            let slot = type_slot[owner].context("INVALID_INPUT: declaring type is unreadable")?;
            let declaring_type = result_types[slot].full_name.clone();
            let token = 0x06000000 | (idx as u32 + 1);
            let type_token = 0x02000000 | (owner as u32 + 1);
            let is_value_type = value_slots[slot];
            work.charge(name.len().saturating_add(4))?; // name bytes and shared owner handles
            Ok(Method {
                impl_flags: row.impl_flags,
                generic_parameters: generic_parameters.remove(&token).unwrap_or_default(),
                token,
                declaring_type,
                declaring_type_token: type_token,
                declaring_type_is_value_type: is_value_type,
                name: name.to_owned(),
                signature: empty_signature(),
                parameters: empty_parameters(),
                return_type: empty_signature(),
                has_this: false,
                signature_flags: 0,
                signature_index: row.signature.get(),
                resolved: false,
                has_body: row.rva != 0 && row.impl_flags & 3 == 0,
                rva: row.rva,
                flags: row.flags,
            })
        })();
        match processed {
            Ok(method) => result_methods.push(method),
            Err(error) => unreadable!("method", Some(0x06000000 | (idx as u32 + 1)), error),
        }
    }
    input.unreadable = unreadable;
    input.types = result_types;
    input.fields = result_fields;
    input.methods = result_methods;
    input.type_kinds = type_kinds;
    input.type_names = names;
    input.metadata_work = work.used();
    Ok(())
}

// Reserve row/index storage before allocation, including Vec growth and identity strings.
fn reserve_metadata(input: &Input<'_>, work: &mut Work) -> Result<()> {
    let id_bytes = input.source_id.len() + 32;
    let word = std::mem::size_of::<usize>();
    let weights = [
        (
            1,
            12 * (std::mem::size_of::<(u32, Arc<String>)>() + word) + 40,
        ),
        (
            2,
            2 * (std::mem::size_of::<TypeDefRow>() + std::mem::size_of::<TypeInfo>())
                + 2 * id_bytes
                + 12 * (std::mem::size_of::<(u32, Arc<String>)>() + word)
                + 64,
        ),
        (
            4,
            2 * (std::mem::size_of::<FieldRow>() + std::mem::size_of::<Field>())
                + 2 * id_bytes
                + std::mem::size_of::<Option<usize>>(),
        ),
        (
            6,
            2 * (std::mem::size_of::<MethodDefRow>() + std::mem::size_of::<Method>())
                + 2 * id_bytes
                + std::mem::size_of::<Option<usize>>(),
        ),
        (
            10,
            2 * std::mem::size_of::<MemberRefRow>()
                + 12 * (std::mem::size_of::<(u32, bool)>() + word),
        ),
        (43, 2 * std::mem::size_of::<MethodSpecRow>()),
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
    input: &Input<'_>,
    work: &mut Work,
    unreadable: &mut Vec<crate::Unreadable>,
) -> Result<BTreeMap<u32, Vec<crate::GenericParameter>>> {
    let tables = input.tables();
    let mut constraints: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (rid, row) in tables.iter::<GenericParamConstraintRow>() {
        let constraint_token = token_of(TableId::GenericParamConstraint, rid);
        let row = match row {
            Ok(row) => row,
            Err(error) => {
                unreadable.push(crate::Unreadable {
                    token: None,
                    kind: "generic_constraint",
                    reason: format!("{error:#}"),
                });
                continue;
            }
        };
        let processed = (|| -> Result<()> {
            work.charge(std::mem::size_of::<(u32, u32)>())?;
            input.check_token(0x2a000000 | row.owner.get(), &[42])?;
            input.check_token(row.constraint.value(), &[1, 2, 27])?;
            constraints
                .entry(row.owner.get())
                .or_default()
                .push(row.constraint.value());
            Ok(())
        })();
        if let Err(error) = processed {
            let reason = format!("{error:#}");
            if reason.contains("RESOURCE_LIMIT") {
                return Err(error);
            }
            unreadable.push(crate::Unreadable {
                token: Some(constraint_token),
                kind: "generic_constraint",
                reason,
            });
        }
    }
    let mut result: BTreeMap<u32, Vec<crate::GenericParameter>> = BTreeMap::new();
    for (rid, row) in tables.iter::<GenericParamRow>() {
        let parameter_token = token_of(TableId::GenericParam, rid);
        let row = match row {
            Ok(row) => row,
            Err(error) => {
                unreadable.push(crate::Unreadable {
                    token: None,
                    kind: "generic_parameter",
                    reason: format!("{error:#}"),
                });
                continue;
            }
        };
        let processed = (|| -> Result<()> {
            let owner = row.owner.value();
            input.check_token(owner, &[2, 6])?;
            let name = input.name(row.name)?;
            work.retain(name.len() + std::mem::size_of::<crate::GenericParameter>())?;
            let items = result.entry(owner).or_default();
            ensure!(
                !items
                    .iter()
                    .any(|parameter| parameter.number == row.number as u32),
                "INVALID_INPUT: duplicate generic parameter"
            );
            items.push(crate::GenericParameter {
                token: parameter_token,
                number: row.number as u32,
                flags: row.flags as u32,
                name: name.to_owned(),
                constraints: constraints.remove(&rid).unwrap_or_default(),
            });
            Ok(())
        })();
        if let Err(error) = processed {
            let reason = format!("{error:#}");
            if reason.contains("RESOURCE_LIMIT") {
                return Err(error);
            }
            unreadable.push(crate::Unreadable {
                token: Some(parameter_token),
                kind: "generic_parameter",
                reason,
            });
        }
    }
    for parameters in result.values_mut() {
        parameters.sort_by_key(|parameter| parameter.number);
    }
    Ok(result)
}

pub(crate) fn load_relationships(input: &mut Input<'_>) -> Result<()> {
    if input.relationships_loaded {
        return Ok(());
    }
    let mut work = Work::new(input.limits.work, input.limits.metadata_bytes);
    work.charge(input.metadata_work)?;
    let tables = input.tables();
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
                .checked_mul(std::mem::size_of::<PropertyMapRow>())
                .and_then(|maps| bytes.checked_add(maps))
        })
        .and_then(|bytes| {
            (input.rows[18] as usize)
                .checked_mul(std::mem::size_of::<EventMapRow>())
                .and_then(|maps| bytes.checked_add(maps))
        })
        .context("RESOURCE_LIMIT: relationship collections")?;
    work.retain(collection_bytes)?;
    let maps = rows::<PropertyMapRow>(tables)?;
    let mut property_owners = vec![None; input.rows[23] as usize];
    let mut seen = BTreeSet::new();
    for (index, row) in maps.iter().enumerate() {
        work.charge(1)?;
        input.check_token(0x02000000 | row.parent.get(), &[2])?;
        ensure!(
            seen.insert(row.parent.get()),
            "INVALID_INPUT: duplicate property owner"
        );
        assign_range(
            &mut property_owners,
            row.property_list.get(),
            maps.get(index + 1)
                .map_or(input.rows[23] + 1, |next| next.property_list.get()),
            (row.parent.get() - 1) as usize,
        )?;
    }
    for (rid, row) in tables.iter::<PropertyRow>() {
        let token = token_of(TableId::Property, rid);
        let row = row?;
        let owner = input
            .types
            .get(property_owners[rid as usize - 1].context("INVALID_INPUT: unowned property")?)
            .context("INVALID_INPUT: property owner")?;
        let bytes = input.blob(row.type_signature)?;
        let parsed = crate::signature::property(input, bytes, &mut work)?;
        let flags = parsed.flags;
        let return_type = parsed.return_type.display(&mut work)?;
        let parameters = crate::signature::display_types(&parsed.parameters, &mut work)?;
        let signature = format!(
            "flags=0x{flags:02x};{return_type}({})",
            parameters.join(",")
        );
        let name = input.name(row.name)?;
        work.charge(name.len() + signature.len() + bytes.len() * 2)?;
        properties.push(crate::Property {
            token,
            declaring_type_token: owner.token,
            name: name.to_owned(),
            signature,
            signature_hex: bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
            flags: row.flags as u32,
            accessors: Vec::new(),
        });
    }
    let maps = rows::<EventMapRow>(tables)?;
    let mut event_owners = vec![None; input.rows[20] as usize];
    seen.clear();
    for (index, row) in maps.iter().enumerate() {
        work.charge(1)?;
        input.check_token(0x02000000 | row.parent.get(), &[2])?;
        ensure!(
            seen.insert(row.parent.get()),
            "INVALID_INPUT: duplicate event owner"
        );
        assign_range(
            &mut event_owners,
            row.event_list.get(),
            maps.get(index + 1)
                .map_or(input.rows[20] + 1, |next| next.event_list.get()),
            (row.parent.get() - 1) as usize,
        )?;
    }
    for (rid, row) in tables.iter::<EventRow>() {
        let token = token_of(TableId::Event, rid);
        let row = row?;
        let owner = input
            .types
            .get(event_owners[rid as usize - 1].context("INVALID_INPUT: unowned event")?)
            .context("INVALID_INPUT: event owner")?;
        input.check_token(row.event_type.value(), &[1, 2, 27])?;
        let name = input.name(row.name)?;
        work.charge(name.len() + 1)?;
        events.push(crate::Event {
            token,
            declaring_type_token: owner.token,
            name: name.to_owned(),
            event_type_token: row.event_type.value(),
            flags: row.event_flags as u32,
            accessors: Vec::new(),
        });
    }
    for (_, row) in tables.iter::<MethodSemanticsRow>() {
        let row = row?;
        work.charge(1)?;
        let token = 0x06000000 | row.method.get();
        input.check_token(token, &[6])?;
        let method = &input.methods[row.method.get() as usize - 1];
        let association = row.association.value();
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
                    && accessor.semantics == row.semantics as u32),
            "INVALID_INPUT: duplicate accessor association"
        );
        work.retain(std::mem::size_of::<crate::Accessor>())?;
        accessors.push(crate::Accessor {
            method_token: token,
            semantics: row.semantics as u32,
        });
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
    input.metadata_work = work.used();
    input.relationships_loaded = true;
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

pub(crate) struct MethodSig {
    pub(crate) flags: u8,
    pub(crate) parameters: Arc<Vec<String>>,
    pub(crate) return_type: Arc<String>,
    pub(crate) has_this: bool,
    pub(crate) vararg: bool,
    generic_count: u32,
    sentinel: Option<usize>,
}
impl MethodSig {
    pub(crate) fn display(&self) -> String {
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

/// Resolve only the requested call signature and validate its operands.
pub(crate) fn call_signature(
    input: &Input<'_>,
    token: u32,
    work: &mut Work,
) -> Result<CallSignature> {
    work.charge(1)?;
    input.check_token(token, &[6, 10, 43])?;
    let rid = token & 0xffffff;
    match token >> 24 {
        6 => {
            let method = input
                .methods
                .get(rid as usize - 1)
                .context("INVALID_INPUT: missing method call token")?;
            // Parse this row's own blob so the projection never depends on whether an
            // unrelated eager pass ran.
            let sig = method_signature(input, input.blob(method.signature_index)?, work)?;
            ensure!(!sig.vararg, "UNSUPPORTED: vararg call signature");
            work.charge(method.declaring_type.len() + method.name.len() + 2)?;
            Ok(CallSignature {
                token,
                name: format!("{}::{}", method.declaring_type, method.name),
                parameters: sig.parameters,
                return_type: sig.return_type,
                has_this: sig.has_this,
                signature_flags: sig.flags,
                declaring_type_token: method.declaring_type_token,
                method_arguments: None,
            })
        }
        10 => {
            ensure!(
                input.member_kind(token)? == Some(true),
                "INVALID_INPUT: call references field"
            );
            let row = input
                .tables()
                .member_ref(rid)
                .context("INVALID_INPUT: MemberRef token")?;
            let sig = method_signature(input, input.blob(row.signature)?, work)?;
            ensure!(!sig.vararg, "UNSUPPORTED: vararg call signature");
            let name = input.name(row.name)?;
            let parent = row.class.value();
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
                .tables()
                .method_spec(rid)
                .context("INVALID_INPUT: MethodSpec token")?;
            let target = row.method.value();
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

pub(crate) fn method_signature(
    input: &Input<'_>,
    bytes: &[u8],
    work: &mut Work,
) -> Result<MethodSig> {
    let parsed = crate::signature::method(input, bytes, work)?;
    let parameters = crate::signature::display_types(&parsed.parameters, work)?;
    let return_type = parsed.return_type.display(work)?;
    work.retain(2 * (std::mem::size_of::<String>() + 2 * std::mem::size_of::<usize>()))?;
    Ok(MethodSig {
        flags: parsed.flags,
        has_this: parsed.flags & 0x20 != 0,
        vararg: matches!(parsed.flags & 15, 5 | 11),
        generic_count: parsed.generic_arity,
        sentinel: parsed.sentinel,
        parameters: Arc::new(parameters),
        return_type: Arc::new(return_type),
    })
}
pub(crate) fn field_signature(input: &Input<'_>, bytes: &[u8], work: &mut Work) -> Result<String> {
    crate::signature::field(input, bytes, work)?.display(work)
}
pub(crate) fn type_signature(input: &Input<'_>, bytes: &[u8], work: &mut Work) -> Result<String> {
    crate::signature::ty(input, bytes, work)?.display(work)
}
pub(crate) fn locals(input: &Input<'_>, bytes: &[u8], work: &mut Work) -> Result<Vec<String>> {
    crate::signature::display_types(&crate::signature::locals(input, bytes, work)?, work)
}
fn generic_arguments(input: &Input<'_>, bytes: &[u8], work: &mut Work) -> Result<Vec<String>> {
    crate::signature::display_types(&crate::signature::arguments(input, bytes, work)?, work)
}
pub(crate) fn type_arguments(
    input: &Input<'_>,
    token: u32,
    work: &mut Work,
) -> Result<Option<Vec<String>>> {
    if token >> 24 != 27 {
        return Ok(None);
    }
    let row = input
        .tables()
        .type_spec(token & 0xffffff)
        .context("UNSUPPORTED: missing owner TypeSpec")?;
    let bytes = input.blob(row.signature)?;
    if bytes.first() != Some(&0x15) {
        return Ok(None);
    }
    let parsed = crate::signature::ty(input, bytes, work)?;
    let crate::signature::TypeSignature::Constructed { arguments, .. } = parsed else {
        unreachable!("generic owner tag checked")
    };
    Ok(Some(crate::signature::display_types(&arguments, work)?))
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

pub(crate) fn strings(input: &Input<'_>) -> Result<Vec<StringRecord>> {
    strings_with_work(
        input,
        &mut Work::new(input.limits.work, input.limits.metadata_bytes),
    )
}

pub(crate) fn strings_with_work(input: &Input<'_>, work: &mut Work) -> Result<Vec<StringRecord>> {
    let bytes = input.metadata.user_strings().data();
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
