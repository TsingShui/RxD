use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use crate::{ClassBody, ClassMethod, Input, input::Work};
use anyhow::{Context, Result, ensure};

/// Resolve only what this class query needs: property/event associations and the
/// signatures of its own (and nested) methods. Unrelated rows are never parsed.
pub(crate) fn prepare(input: &mut Input, root_token: u32) -> Result<()> {
    {
        let root = input
            .types
            .iter()
            .find(|ty| ty.token == root_token)
            .context("INVALID_QUERY: type does not exist")?;
        ensure!(
            root.token != 0x02000001 || root.name != "<Module>",
            "INVALID_QUERY: synthetic module is not a class"
        );
    }
    crate::metadata::load_relationships(input)?;
    let mut work = Work::new(input.limits.work, input.limits.metadata_bytes);
    work.charge(input.metadata_work)?;
    let selected = selected_types(input, root_token, &mut work)?;
    let mut owned = Vec::new();
    for (index, item) in input.methods.iter().enumerate() {
        work.charge(1)?;
        if selected.contains(&item.declaring_type_token) {
            owned.push(index);
        }
    }
    for index in owned {
        input.resolve_method(index)?;
    }
    Ok(())
}

/// Field signatures are only needed when the caller emits the member inventory
/// (`--raw`); the default C# answer never reads them.
pub(crate) fn prepare_fields(input: &mut Input, root_token: u32) -> Result<()> {
    let mut work = Work::new(input.limits.work, input.limits.metadata_bytes);
    work.charge(input.metadata_work)?;
    let selected = selected_types(input, root_token, &mut work)?;
    let mut owned = Vec::new();
    for (index, field) in input.fields.iter().enumerate() {
        work.charge(1)?;
        if selected.contains(&field.declaring_type_token) {
            owned.push(index);
        }
    }
    for index in owned {
        input.resolve_field(index)?;
    }
    Ok(())
}

pub(crate) fn build<'a>(input: &'a Input, root_token: u32) -> Result<ClassBody<'a>> {
    let mut work = Work::new(input.limits.work, input.limits.metadata_bytes);
    work.charge(input.metadata_work)?;
    let selected = selected_types(input, root_token, &mut work)?;
    let root = input
        .types
        .iter()
        .find(|ty| ty.token == root_token)
        .context("INVALID_QUERY: type does not exist")?;
    let types: Vec<_> = input
        .types
        .iter()
        .filter(|ty| selected.contains(&ty.token))
        .collect();
    let fields: Vec<_> = input
        .fields
        .iter()
        .filter(|field| selected.contains(&field.declaring_type_token))
        .collect();
    work.charge(input.fields.len() + input.properties.len() + input.events.len())?;
    let properties: Vec<_> = input
        .properties
        .iter()
        .filter(|property| selected.contains(&property.declaring_type_token))
        .collect();
    let events: Vec<_> = input
        .events
        .iter()
        .filter(|event| selected.contains(&event.declaring_type_token))
        .collect();
    work.charge(input.methods.len())?;
    // The serialized class projection is output, so bound it with the output
    // envelope. The CLI's complete-output writer remains the final authority.
    let mut footprint = Counter {
        remaining: input.limits.output_bytes,
    };
    for ty in &types {
        serde_json::to_writer(&mut footprint, ty)?;
    }
    for field in &fields {
        serde_json::to_writer(&mut footprint, field)?;
    }
    for property in &properties {
        serde_json::to_writer(&mut footprint, property)?;
    }
    for event in &events {
        serde_json::to_writer(&mut footprint, event)?;
    }
    let mut methods = Vec::new();
    for method in &input.methods {
        work.charge(1)?;
        if !selected.contains(&method.declaring_type_token) {
            continue;
        }
        serde_json::to_writer(&mut footprint, method)
            .context("RESOURCE_LIMIT: class metadata output")?;
        let body = if method.has_body {
            let body = crate::il::body_with_budget(input, method, input.limits.work - work.used())?;
            work.charge(body.work_used)?;
            serde_json::to_writer(&mut footprint, &body)
                .context("RESOURCE_LIMIT: aggregate class IL output")?;
            Some(body)
        } else {
            None
        };
        methods.push(ClassMethod { method, body });
    }
    Ok(ClassBody {
        root,
        types,
        fields,
        properties,
        events,
        methods,
        work_used: work.used(),
    })
}

/// The selected type plus every type nested under it, with cycle detection.
fn selected_types(input: &Input, root_token: u32, work: &mut Work) -> Result<BTreeSet<u32>> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for ty in &input.types {
        work.charge(1)?;
        if let Some(parent) = ty.declaring_type_token {
            children.entry(parent).or_default().push(ty.token);
        }
    }
    let mut selected = BTreeSet::from([root_token]);
    let mut frontier = vec![root_token];
    while let Some(parent) = frontier.pop() {
        for child in children.get(&parent).into_iter().flatten() {
            work.charge(1)?;
            ensure!(
                selected.insert(*child),
                "INVALID_INPUT: cyclic nested ownership"
            );
            frontier.push(*child);
        }
    }
    Ok(selected)
}

struct Counter {
    remaining: usize,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::other("RESOURCE_LIMIT: class output bytes"));
        }
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
