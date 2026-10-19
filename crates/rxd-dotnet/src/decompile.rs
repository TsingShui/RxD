//! Adapt the optional native C# engine; callers always retain the original IL/EH.
use anyhow::{Result, ensure};

use crate::input::Work;
use crate::{ClassBody, ClassDecompilation, Decompilation, Input, Limits, Method, MethodBody};

pub(super) fn fallback(reason: impl Into<String>) -> Decompilation {
    Decompilation {
        representation: "il".to_owned(),
        decompiled: false,
        reason: Some(reason.into()),
        warnings: vec![],
        code: None,
        mapping: vec![],
    }
}

pub(super) fn class_fallback(reason: impl Into<String>) -> ClassDecompilation {
    ClassDecompilation {
        representation: "il".into(),
        decompiled: false,
        reason: Some(reason.into()),
        warnings: vec![],
        code: None,
        mapping: vec![],
    }
}

pub fn render_class(
    input: &Input,
    class: &ClassBody<'_>,
    limits: Limits,
) -> Result<ClassDecompilation> {
    ensure!(
        input.file_bytes <= limits.input_bytes,
        "RESOURCE_LIMIT: input bytes"
    );
    let mut work = Work::new(limits.work);
    work.charge(class.work_used)?;
    for item in &class.methods {
        if let Some(body) = &item.body {
            ensure!(
                body.instructions.len() <= limits.instructions,
                "RESOURCE_LIMIT: IL instruction budget exceeded"
            );
            validate_operands(input, item.method, body, &mut work)?;
        }
    }
    #[cfg(feature = "csharp-decompiler")]
    return crate::native::render_class(input, class, limits);
    #[cfg(not(feature = "csharp-decompiler"))]
    Ok(class_fallback("class_engine_unavailable"))
}

pub fn render(
    input: &Input,
    method: &Method,
    body: &MethodBody,
    limits: Limits,
) -> Result<Decompilation> {
    ensure!(
        input.file_bytes <= limits.input_bytes,
        "RESOURCE_LIMIT: input bytes"
    );
    ensure!(
        body.instructions.len() <= limits.instructions,
        "RESOURCE_LIMIT: IL instruction budget exceeded"
    );
    let entities = input
        .types
        .len()
        .saturating_add(input.fields.len())
        .saturating_add(input.methods.len());
    ensure!(
        entities <= limits.entities,
        "RESOURCE_LIMIT: module entity budget exceeded"
    );
    // Admission uses analysis units, not file bytes (padding is not analysis work).
    ensure!(
        body.work_used
            .saturating_add(body.instructions.len())
            .saturating_add(entities)
            <= limits.work,
        "RESOURCE_LIMIT: decompilation work budget exceeded"
    );
    ensure!(
        body.il_hex.len() <= limits.output_bytes,
        "RESOURCE_LIMIT: original IL output budget exceeded"
    );
    let mut work = Work::new(limits.work);
    work.charge(body.work_used.saturating_add(entities))?;
    validate_operands(input, method, body, &mut work)?;
    #[cfg(feature = "csharp-decompiler")]
    return crate::native::render(input, method, body, limits);
    #[cfg(not(feature = "csharp-decompiler"))]
    {
        let _ = method;
        Ok(fallback("engine_unavailable"))
    }
}

// Validate selected original operand signatures independently of engine availability.
fn validate_operands(
    input: &Input,
    method: &Method,
    body: &MethodBody,
    work: &mut Work,
) -> Result<()> {
    for instruction in &body.instructions {
        work.charge(1)?;
        if let crate::Operand::Token(token) = instruction.operand {
            let result = match token >> 24 {
                4 => input
                    .field_signature_with_work(token, method, work)
                    .map(|_| ()),
                10 if matches!(
                    instruction.mnemonic.as_str(),
                    "ldfld" | "ldflda" | "stfld" | "ldsfld" | "ldsflda" | "stsfld"
                ) =>
                {
                    input
                        .field_signature_with_work(token, method, work)
                        .map(|_| ())
                }
                27 => input.type_signature_with_work(token, work).map(|_| ()),
                _ => Ok(()),
            };
            if let Err(error) = result
                && !format!("{error:#}").contains("UNSUPPORTED:")
            {
                return Err(error);
            }
        }
    }
    Ok(())
}
