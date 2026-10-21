use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use crate::{ClassBody, ClassMethod, Input, TypeInfo, input::Work};
use anyhow::{Context, Result, ensure};

pub(crate) fn body<'a>(input: &'a Input, root: &'a TypeInfo) -> Result<ClassBody<'a>> {
    ensure!(
        root.token != 0x02000001 || root.name != "<Module>",
        "INVALID_QUERY: synthetic module is not a class"
    );
    let mut work = Work::new(input.limits.work, input.limits.metadata_bytes);
    work.charge(input.metadata_work)?;
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for ty in &input.types {
        work.charge(1)?;
        if let Some(parent) = ty.declaring_type_token {
            children.entry(parent).or_default().push(ty.token);
        }
    }
    let mut selected = BTreeSet::from([root.token]);
    let mut frontier = vec![root.token];
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
