//! Keep upstream structural/heap/reference checks while caching repeated graph checks.
use dotscope::{
    CilAssemblyView, ValidationConfig,
    metadata::validation::{RawHeapValidator, RawTableValidator, RawValidator, ValidationEngine},
};

#[path = "../vendor/dotscope-validation/token.rs"]
mod token;

pub(crate) fn validate(view: &CilAssemblyView, config: ValidationConfig) -> dotscope::Result<()> {
    let validators: Vec<Box<dyn RawValidator>> = vec![
        Box::new(RawTableValidator::new()),
        Box::new(RawHeapValidator::new()),
        Box::new(token::RawTokenValidator::new()),
    ];
    let engine = ValidationEngine::new(view, config)?;
    engine
        .validate_raw_stage(view, None, &validators)?
        .into_result()
}
