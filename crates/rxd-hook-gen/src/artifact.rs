use crate::{Diagnostic, ErrorKind, Language, Limits, Result, Stage, limits::check};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Artifact {
    Source(SourceArtifact),
    Extension(ExtensionArtifacts),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceArtifact {
    pub language: Language,
    pub source: String,
}

impl SourceArtifact {
    pub fn new(language: Language, source: String, limits: Limits) -> Result<Self> {
        limits.validate()?;
        limits.check_source(source.len())?;
        check(
            source.len(),
            limits.artifact_bytes,
            Stage::Render,
            "artifact byte budget exceeded",
        )?;
        Ok(Self { language, source })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactFile {
    pub name: String,
    pub contents: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionArtifacts {
    pub files: Vec<ArtifactFile>,
}

impl ExtensionArtifacts {
    pub fn new(files: Vec<ArtifactFile>, limits: Limits) -> Result<Self> {
        let artifacts = Self { files };
        artifacts.validate(limits)?;
        Ok(artifacts)
    }

    /// Publication must revalidate because artifact fields are caller-editable.
    /// Files are local single-component names; nested paths and symlinks are never artifacts.
    pub fn validate(&self, limits: Limits) -> Result<()> {
        limits.validate()?;
        check(
            self.files.len(),
            limits.artifact_files,
            Stage::Render,
            "artifact file budget exceeded",
        )?;
        let mut names = BTreeSet::new();
        let mut total = 0usize;
        for file in &self.files {
            if file.name.is_empty()
                || file.name == "."
                || file.name == ".."
                || file
                    .name
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
                || !names.insert(&file.name)
            {
                return Err(Diagnostic::new(
                    ErrorKind::InvalidInput,
                    Stage::Render,
                    "invalid or duplicate artifact filename",
                ));
            }
            total = total.checked_add(file.contents.len()).ok_or_else(|| {
                Diagnostic::new(
                    ErrorKind::ResourceLimit,
                    Stage::Render,
                    "artifact byte count overflow",
                )
            })?;
            check(
                total,
                limits.artifact_bytes,
                Stage::Render,
                "artifact byte budget exceeded",
            )?;
            if file.name.ends_with(".js") {
                limits.check_source(file.contents.len())?;
            }
        }
        if !names.contains(&"manifest.json".to_string()) {
            return Err(Diagnostic::new(
                ErrorKind::InvalidInput,
                Stage::Render,
                "extension requires manifest.json",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationReceipt {
    pub directory: PathBuf,
    pub files: Vec<PathBuf>,
    pub bytes: usize,
}
