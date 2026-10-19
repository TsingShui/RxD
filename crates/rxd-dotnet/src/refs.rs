//! Query-local, bounded proof of metadata identities in one immutable module.
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use anyhow::{Context, Result, ensure};
use dotscope::metadata::tables::{
    AssemblyRaw, AssemblyRefRaw, FieldRaw, MemberRefRaw, MethodDefRaw, MethodSpecRaw, ModuleRefRaw,
    TypeRefRaw, TypeSpecRaw,
};
use serde::Serialize;

use crate::{
    Input,
    input::Work,
    signature::{self, MethodSignature, TypeSignature},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScopeIdentity {
    Module,
    Assembly {
        name: String,
        version: [u32; 4],
        culture: String,
        flags: u32,
        key: Vec<u8>,
    },
    ModuleRef {
        name: String,
    },
    Nested {
        parent_token: u32,
        parent_scope: Box<ScopeIdentity>,
    },
    Unresolved {
        token: u32,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct SemanticTarget {
    pub target_kind: &'static str,
    pub original_token: u32,
    pub canonical_token: Option<u32>,
    pub canonical_id: Option<String>,
    pub resolution_status: String,
    pub scope: ScopeIdentity,
    pub owner_token: u32,
    pub original_owner_token: u32,
    pub owner_name: String,
    pub name: String,
    pub signature: Option<MethodSignature>,
    pub type_signature: Option<TypeSignature>,
    pub owner_arguments: Vec<TypeSignature>,
    pub method_arguments: Vec<TypeSignature>,
}

#[derive(Clone)]
struct Owner {
    token: u32,
    definition: Option<u32>,
    arguments: Vec<TypeSignature>,
    scope: ScopeIdentity,
    status: &'static str,
}

pub(crate) struct Resolver<'a> {
    input: &'a Input,
    pub(crate) work: Work,
    methods: BTreeMap<u32, Rc<SemanticTarget>>,
    fields: BTreeMap<u32, Rc<SemanticTarget>>,
    type_targets: BTreeMap<u32, Rc<SemanticTarget>>,
    field_candidates: BTreeMap<(u32, String), Vec<u32>>,
    owners: BTreeMap<u32, Owner>,
    active: BTreeSet<u32>,
    named: BTreeMap<(ScopeIdentity, String, String), u32>,
    candidates: BTreeMap<(u32, String), Vec<u32>>,
    types: BTreeMap<(Option<u32>, String, String), Vec<u32>>,
}

impl Input {
    pub fn method_target(&self, token: u32) -> Result<SemanticTarget> {
        let mut resolver = Resolver::new(self, self.metadata_work)?;
        let target = resolver.method(token)?;
        target.charge_clone(&mut resolver.work)?;
        Ok((*target).clone())
    }
}

impl ScopeIdentity {
    fn charge_clone(&self, work: &mut Work) -> Result<()> {
        work.charge(std::mem::size_of::<Self>())?;
        match self {
            Self::Assembly {
                name, culture, key, ..
            } => work.charge(name.len() + culture.len() + key.len()),
            Self::ModuleRef { name } => work.charge(name.len()),
            Self::Nested { parent_scope, .. } => parent_scope.charge_clone(work),
            _ => Ok(()),
        }
    }
}
impl SemanticTarget {
    fn charge_clone(&self, work: &mut Work) -> Result<()> {
        work.charge(
            std::mem::size_of::<Self>()
                + self.canonical_id.as_ref().map_or(0, String::len)
                + self.resolution_status.len()
                + self.owner_name.len()
                + self.name.len(),
        )?;
        self.scope.charge_clone(work)?;
        if let Some(signature) = &self.signature {
            signature.charge_clone(work, 0)?;
        }
        if let Some(signature) = &self.type_signature {
            signature.charge_clone(work, 0)?;
        }
        signature::charge_types(&self.owner_arguments, work, 0)?;
        signature::charge_types(&self.method_arguments, work, 0)
    }
}
impl Owner {
    fn bounded_clone(&self, work: &mut Work) -> Result<Self> {
        work.charge(std::mem::size_of::<Self>())?;
        self.scope.charge_clone(work)?;
        signature::charge_types(&self.arguments, work, 0)?;
        Ok(self.clone())
    }
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(input: &'a Input, used: usize) -> Result<Self> {
        let mut work = Work::new(input.limits.work);
        work.charge(used)?;
        let mut candidates: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for method in &input.methods {
            work.charge(1 + method.name.len())?;
            candidates
                .entry((method.declaring_type_token, method.name.clone()))
                .or_default()
                .push(method.token);
        }
        let mut field_candidates: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for field in &input.fields {
            work.charge(1 + field.name.len())?;
            field_candidates
                .entry((field.declaring_type_token, field.name.clone()))
                .or_default()
                .push(field.token);
        }
        let mut types: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for ty in &input.types {
            work.charge(1 + ty.name.len() + ty.namespace.len())?;
            types
                .entry((
                    ty.declaring_type_token,
                    ty.namespace.clone(),
                    ty.name.clone(),
                ))
                .or_default()
                .push(ty.token);
        }
        Ok(Self {
            input,
            work,
            methods: BTreeMap::new(),
            fields: BTreeMap::new(),
            type_targets: BTreeMap::new(),
            field_candidates,
            owners: BTreeMap::new(),
            active: BTreeSet::new(),
            named: BTreeMap::new(),
            candidates,
            types,
        })
    }
    fn enter(&mut self, token: u32) -> Result<()> {
        self.work.charge(1)?;
        ensure!(
            self.active.len() < 64,
            "RESOURCE_LIMIT: semantic resolution depth"
        );
        ensure!(
            self.active.insert(token),
            "INVALID_INPUT: cyclic semantic resolution"
        );
        Ok(())
    }
    pub(crate) fn method(&mut self, token: u32) -> Result<Rc<SemanticTarget>> {
        self.input.check_token(token, &[6, 10, 43])?;
        self.work.charge(1)?;
        if let Some(target) = self.methods.get(&token) {
            return Ok(target.clone());
        }
        self.enter(token)?;
        let result = Rc::new(self.method_inner(token)?);
        self.active.remove(&token);
        self.methods.insert(token, result.clone());
        Ok(result)
    }
    fn method_inner(&mut self, token: u32) -> Result<SemanticTarget> {
        if token >> 24 == 43 {
            let row = self
                .input
                .view
                .tables()
                .and_then(|t| t.table::<MethodSpecRaw>())
                .context("INVALID_INPUT: MethodSpec table")?
                .get(token & 0xffffff)?
                .context("INVALID_INPUT: MethodSpec row")?;
            let base = self.method(row.method.token.value())?;
            base.charge_clone(&mut self.work)?;
            let mut target = (*base).clone();
            let arguments = signature::arguments(
                self.input,
                self.input.blob(row.instantiation)?,
                &mut self.work,
            )?;
            ensure!(
                target.signature.as_ref().unwrap().flags & 0x10 != 0
                    && target.signature.as_ref().unwrap().generic_arity > 0
                    && arguments.len() == target.signature.as_ref().unwrap().generic_arity as usize,
                "INVALID_INPUT: MethodSpec argument arity"
            );
            target.original_token = token;
            target.method_arguments = arguments
                .into_iter()
                .map(|arg| self.normalize(arg, None, 0))
                .collect::<Result<_>>()?;
            return Ok(target);
        }
        if token >> 24 == 6 {
            let row = self
                .input
                .view
                .tables()
                .and_then(|t| t.table::<MethodDefRaw>())
                .context("INVALID_INPUT: MethodDef table")?
                .get(token & 0xffffff)?
                .context("INVALID_INPUT: MethodDef row")?;
            let method = &self.input.methods[(token & 0xffffff) as usize - 1];
            let signature =
                signature::method(self.input, self.input.blob(row.signature)?, &mut self.work)?;
            let signature = self.normalize_method(signature, None, 0)?;
            return Ok(SemanticTarget {
                target_kind: "method",
                type_signature: None,
                original_token: token,
                canonical_token: Some(token),
                canonical_id: Some(method.id.clone()),
                resolution_status: "resolved".into(),
                scope: ScopeIdentity::Module,
                owner_token: method.declaring_type_token,
                original_owner_token: method.declaring_type_token,
                owner_name: method.declaring_type.clone(),
                name: method.name.clone(),
                signature: Some(signature),
                owner_arguments: Vec::new(),
                method_arguments: Vec::new(),
            });
        }
        ensure!(
            self.input.member_kinds.get(&token) == Some(&true),
            "INVALID_QUERY: semantic method selector references field"
        );
        let row = self
            .input
            .view
            .tables()
            .and_then(|t| t.table::<MemberRefRaw>())
            .context("INVALID_INPUT: MemberRef table")?
            .get(token & 0xffffff)?
            .context("INVALID_INPUT: MemberRef row")?;
        let owner_token = row.class.token.value();
        let owner = if owner_token >> 24 == 6 {
            // A MethodDef parent denotes a vararg callsite; optional arguments are
            // preserved but no ordinary full-signature equality is inferred.
            Owner {
                token: owner_token,
                definition: None,
                arguments: Vec::new(),
                scope: ScopeIdentity::Unresolved { token: owner_token },
                status: "unresolved",
            }
        } else {
            self.owner(owner_token)?
        };
        let name = self.input.name(row.name)?.to_owned();
        self.work.charge(name.len())?;
        let signature =
            signature::method(self.input, self.input.blob(row.signature)?, &mut self.work)?;
        let signature = self.normalize_method(signature, Some(&owner.arguments), 0)?;
        signature::charge_types(&owner.arguments, &mut self.work, 0)?;
        owner.scope.charge_clone(&mut self.work)?;
        let mut target = SemanticTarget {
            target_kind: "method",
            type_signature: None,
            original_token: token,
            canonical_token: None,
            canonical_id: None,
            resolution_status: owner.status.into(),
            scope: owner.scope.clone(),
            owner_token: owner.token,
            original_owner_token: owner_token,
            owner_name: self.owner_name(&owner)?,
            name: name.clone(),
            signature: Some(signature),
            owner_arguments: owner.arguments.clone(),
            method_arguments: Vec::new(),
        };
        if let Some(definition) = owner.definition {
            let candidates = self
                .candidates
                .get(&(definition, name))
                .cloned()
                .unwrap_or_default();
            let mut matches = Vec::new();
            for candidate in candidates {
                self.work.charge(1)?;
                let row = self
                    .input
                    .view
                    .tables()
                    .and_then(|t| t.table::<MethodDefRaw>())
                    .context("INVALID_INPUT: MethodDef table")?
                    .get(candidate & 0xffffff)?
                    .context("INVALID_INPUT: candidate method")?;
                let signature =
                    signature::method(self.input, self.input.blob(row.signature)?, &mut self.work)?;
                let signature = self.normalize_method(signature, Some(&owner.arguments), 0)?;
                if Some(signature) == target.signature {
                    matches.push(candidate);
                }
            }
            target.resolution_status = match matches.as_slice() {
                [definition] => {
                    target.canonical_token = Some(*definition);
                    target.canonical_id = Some(self.input.id("method", *definition));
                    "resolved"
                }
                [] => "unresolved",
                _ => "ambiguous",
            }
            .into();
        }
        Ok(target)
    }
    fn field(&mut self, token: u32) -> Result<Rc<SemanticTarget>> {
        self.input.check_token(token, &[4, 10])?;
        self.work.charge(1)?;
        if let Some(target) = self.fields.get(&token) {
            return Ok(target.clone());
        }
        self.enter(token)?;
        let (owner_token, name, bytes) = if token >> 24 == 4 {
            let row = self
                .input
                .view
                .tables()
                .and_then(|t| t.table::<FieldRaw>())
                .context("INVALID_INPUT: Field table")?
                .get(token & 0xffffff)?
                .context("INVALID_INPUT: Field row")?;
            let field = &self.input.fields[(token & 0xffffff) as usize - 1];
            (
                field.declaring_type_token,
                field.name.clone(),
                self.input.blob(row.signature)?,
            )
        } else {
            ensure!(
                self.input.member_kinds.get(&token) == Some(&false),
                "INVALID_QUERY: field target required"
            );
            let row = self
                .input
                .view
                .tables()
                .and_then(|t| t.table::<MemberRefRaw>())
                .context("INVALID_INPUT: MemberRef table")?
                .get(token & 0xffffff)?
                .context("INVALID_INPUT: MemberRef row")?;
            (
                row.class.token.value(),
                self.input.name(row.name)?.to_owned(),
                self.input.blob(row.signature)?,
            )
        };
        self.work.charge(name.len())?;
        let owner = self.owner(owner_token)?;
        let signature = signature::field(self.input, bytes, &mut self.work)?;
        let signature = self.normalize(signature, Some(&owner.arguments), 0)?;
        signature::charge_types(&owner.arguments, &mut self.work, 0)?;
        owner.scope.charge_clone(&mut self.work)?;
        let mut target = SemanticTarget {
            target_kind: "field",
            original_token: token,
            canonical_token: None,
            canonical_id: None,
            resolution_status: owner.status.into(),
            scope: owner.scope.clone(),
            owner_token: owner.token,
            original_owner_token: owner_token,
            owner_name: self.owner_name(&owner)?,
            name: name.clone(),
            signature: None,
            type_signature: Some(signature),
            owner_arguments: owner.arguments.clone(),
            method_arguments: Vec::new(),
        };
        if token >> 24 == 4 {
            target.canonical_token = Some(token);
            target.canonical_id = Some(self.input.id("field", token));
        } else if let Some(definition) = owner.definition {
            let candidates = self
                .field_candidates
                .get(&(definition, name))
                .cloned()
                .unwrap_or_default();
            let mut matches = Vec::new();
            for candidate in candidates {
                self.work.charge(1)?;
                let row = self
                    .input
                    .view
                    .tables()
                    .and_then(|t| t.table::<FieldRaw>())
                    .context("INVALID_INPUT: Field table")?
                    .get(candidate & 0xffffff)?
                    .context("INVALID_INPUT: candidate field")?;
                let signature =
                    signature::field(self.input, self.input.blob(row.signature)?, &mut self.work)?;
                let signature = self.normalize(signature, Some(&owner.arguments), 0)?;
                if Some(signature) == target.type_signature {
                    matches.push(candidate);
                }
            }
            target.resolution_status = match matches.as_slice() {
                [definition] => {
                    target.canonical_token = Some(*definition);
                    target.canonical_id = Some(self.input.id("field", *definition));
                    "resolved"
                }
                [] => "unresolved",
                _ => "ambiguous",
            }
            .into();
        }
        self.active.remove(&token);
        let target = Rc::new(target);
        self.fields.insert(token, target.clone());
        Ok(target)
    }
    fn ty(&mut self, token: u32) -> Result<Rc<SemanticTarget>> {
        self.input.check_token(token, &[1, 2, 27])?;
        self.work.charge(1)?;
        if let Some(target) = self.type_targets.get(&token) {
            return Ok(target.clone());
        }
        // owner() and normalize() share the bounded recursion/cycle guards.
        let owner = self.owner(token)?;
        let signature = if token >> 24 == 27 {
            let row = self
                .input
                .view
                .tables()
                .and_then(|t| t.table::<TypeSpecRaw>())
                .context("INVALID_INPUT: TypeSpec table")?
                .get(token & 0xffffff)?
                .context("INVALID_INPUT: TypeSpec row")?;
            let signature =
                signature::ty(self.input, self.input.blob(row.signature)?, &mut self.work)?;
            self.normalize(signature, None, 0)?
        } else {
            TypeSignature::Named {
                element: if self.input.type_kinds.get(&owner.token) == Some(&true) {
                    0x11
                } else {
                    0x12
                },
                token: owner.token,
            }
        };
        let named = matches!(
            signature,
            TypeSignature::Named { .. } | TypeSignature::Constructed { .. }
        );
        let canonical = named.then_some(owner.definition).flatten();
        let name = self.owner_name(&owner)?;
        let target = Rc::new(SemanticTarget {
            target_kind: "type",
            original_token: token,
            canonical_token: canonical,
            canonical_id: canonical.map(|token| self.input.id("type", token)),
            resolution_status: if named { owner.status } else { "unresolved" }.into(),
            scope: owner.scope.clone(),
            owner_token: owner.token,
            original_owner_token: token,
            owner_name: name.clone(),
            name,
            signature: None,
            type_signature: Some(signature),
            owner_arguments: owner.arguments,
            method_arguments: Vec::new(),
        });
        self.type_targets.insert(token, target.clone());
        Ok(target)
    }
    fn kind(&self, token: u32) -> Option<&'static str> {
        match token >> 24 {
            1 | 2 | 27 => Some("type"),
            4 => Some("field"),
            6 | 43 => Some("method"),
            10 => self
                .input
                .member_kinds
                .get(&token)
                .map(|method| if *method { "method" } else { "field" }),
            _ => None,
        }
    }
    fn target(&mut self, token: u32) -> Result<Rc<SemanticTarget>> {
        match self.kind(token) {
            Some("method") => self.method(token),
            Some("field") => self.field(token),
            Some("type") => self.ty(token),
            _ => anyhow::bail!("INVALID_QUERY: semantic metadata target required"),
        }
    }
    fn owner(&mut self, token: u32) -> Result<Owner> {
        self.work.charge(1)?;
        if let Some(owner) = self.owners.get(&token) {
            return owner.bounded_clone(&mut self.work);
        }
        self.enter(token)?;
        let owner = self.owner_inner(token)?;
        self.active.remove(&token);
        self.owners
            .insert(token, owner.bounded_clone(&mut self.work)?);
        Ok(owner)
    }
    fn owner_inner(&mut self, token: u32) -> Result<Owner> {
        if token >> 24 == 2 {
            self.input.check_token(token, &[2])?;
            return Ok(Owner {
                token,
                definition: Some(token),
                arguments: Vec::new(),
                scope: ScopeIdentity::Module,
                status: "resolved",
            });
        }
        if token >> 24 == 26 {
            let row = self
                .input
                .view
                .tables()
                .and_then(|t| t.table::<ModuleRefRaw>())
                .context("INVALID_INPUT: ModuleRef table")?
                .get(token & 0xffffff)?
                .context("INVALID_INPUT: ModuleRef row")?;
            let name = self.input.name(row.name)?.to_owned();
            self.work.charge(name.len())?;
            return Ok(Owner {
                token,
                definition: None,
                arguments: Vec::new(),
                scope: ScopeIdentity::ModuleRef { name },
                status: "external",
            });
        }
        if token >> 24 == 27 {
            let row = self
                .input
                .view
                .tables()
                .and_then(|t| t.table::<TypeSpecRaw>())
                .context("INVALID_INPUT: TypeSpec table")?
                .get(token & 0xffffff)?
                .context("INVALID_INPUT: TypeSpec row")?;
            let signature =
                signature::ty(self.input, self.input.blob(row.signature)?, &mut self.work)?;
            return match signature {
                TypeSignature::Constructed {
                    element,
                    token: base,
                    arguments,
                    ..
                } => {
                    let mut owner = self.owner(base)?;
                    self.validate_kind(element, &owner)?;
                    if !owner.arguments.is_empty() {
                        owner.definition = None;
                        owner.status = "unresolved";
                        owner.token = token;
                        owner.scope = ScopeIdentity::Unresolved { token };
                    }
                    owner.arguments = arguments
                        .into_iter()
                        .map(|arg| self.normalize(arg, None, 0))
                        .collect::<Result<_>>()?;
                    if let Some(definition) = owner.definition {
                        let ty = &self.input.types[(definition & 0xffffff) as usize - 1];
                        ensure!(
                            owner.arguments.len() == ty.generic_parameters.len(),
                            "INVALID_INPUT: constructed owner arity"
                        );
                    }
                    Ok(owner)
                }
                TypeSignature::Named { element, token } => {
                    let owner = self.owner(token)?;
                    self.validate_kind(element, &owner)?;
                    Ok(owner)
                }
                other => {
                    // Resolve every embedded token even when this expression has
                    // no named declaring type, so cycles are never hidden.
                    self.normalize(other, None, 0)?;
                    Ok(Owner {
                        token,
                        definition: None,
                        arguments: Vec::new(),
                        scope: ScopeIdentity::Unresolved { token },
                        status: "unresolved",
                    })
                }
            };
        }
        self.input.check_token(token, &[1])?;
        let row = self
            .input
            .view
            .tables()
            .and_then(|t| t.table::<TypeRefRaw>())
            .context("INVALID_INPUT: TypeRef table")?
            .get(token & 0xffffff)?
            .context("INVALID_INPUT: TypeRef row")?;
        let name = self.input.name(row.type_name)?.to_owned();
        let namespace = self.input.name(row.type_namespace)?.to_owned();
        self.work.charge(name.len() + namespace.len())?;
        let scope_token = row.resolution_scope.token.value();
        let (scope, parent, local, unresolved_status) = match scope_token >> 24 {
            0 if scope_token == 1 => (ScopeIdentity::Module, None, true, "unresolved"),
            1 => {
                let parent = self.owner(scope_token)?;
                (
                    ScopeIdentity::Nested {
                        parent_token: parent.token,
                        parent_scope: Box::new(parent.scope.clone()),
                    },
                    parent.definition,
                    parent.definition.is_some(),
                    parent.status,
                )
            }
            35 => {
                let scope = self.assembly_scope(scope_token)?;
                let own = self.own_assembly_scope()?;
                let local = own.as_ref().is_some_and(|own| *own == scope);
                let status = if own
                    .as_ref()
                    .is_some_and(|own| unresolved_key_relation(own, &scope))
                {
                    "unresolved"
                } else {
                    "external"
                };
                (
                    if local { ScopeIdentity::Module } else { scope },
                    None,
                    local,
                    status,
                )
            }
            26 => (self.owner(scope_token)?.scope, None, false, "external"),
            _ => (
                ScopeIdentity::Unresolved { token: scope_token },
                None,
                false,
                "unresolved",
            ),
        };
        let definitions = if local {
            self.types
                .get(&(parent, namespace.clone(), name.clone()))
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let (definition, status) = match definitions.as_slice() {
            [definition] => (Some(*definition), "resolved"),
            [] if local => (None, "unresolved"),
            [] => (None, unresolved_status),
            _ => (None, "ambiguous"),
        };
        let canonical = if let Some(definition) = definition {
            definition
        } else {
            *self
                .named
                .entry((scope.clone(), namespace, name))
                .or_insert(token)
        };
        Ok(Owner {
            token: canonical,
            definition,
            arguments: Vec::new(),
            scope,
            status,
        })
    }
    fn owner_name(&mut self, owner: &Owner) -> Result<String> {
        let name = if let Some(definition) = owner.definition {
            self.input.types[(definition & 0xffffff) as usize - 1]
                .full_name
                .clone()
        } else if owner.token >> 24 == 1 {
            let row = self
                .input
                .view
                .tables()
                .and_then(|t| t.table::<TypeRefRaw>())
                .context("INVALID_INPUT: owner TypeRef table")?
                .get(owner.token & 0xffffff)?
                .context("INVALID_INPUT: owner TypeRef row")?;
            let namespace = self.input.name(row.type_namespace)?;
            let name = self.input.name(row.type_name)?;
            if namespace.is_empty() {
                name.to_string()
            } else {
                format!("{namespace}.{name}")
            }
        } else {
            format!("token(0x{:08x})", owner.token)
        };
        self.work.charge(name.len())?;
        Ok(name)
    }
    fn validate_kind(&self, element: u8, owner: &Owner) -> Result<()> {
        if let Some(definition) = owner.definition {
            let expected = if self.input.type_kinds[&definition] {
                0x11
            } else {
                0x12
            };
            ensure!(
                element == expected,
                "INVALID_INPUT: signature class/valuetype contradicts local TypeDef"
            );
        }
        Ok(())
    }
    fn assembly_scope(&mut self, token: u32) -> Result<ScopeIdentity> {
        let row = self
            .input
            .view
            .tables()
            .and_then(|t| t.table::<AssemblyRefRaw>())
            .context("INVALID_INPUT: AssemblyRef table")?
            .get(token & 0xffffff)?
            .context("INVALID_INPUT: AssemblyRef row")?;
        self.scope(
            row.name,
            row.culture,
            [
                row.major_version,
                row.minor_version,
                row.build_number,
                row.revision_number,
            ],
            row.flags,
            row.public_key_or_token,
        )
    }
    fn own_assembly_scope(&mut self) -> Result<Option<ScopeIdentity>> {
        let Some(row) = self
            .input
            .view
            .tables()
            .and_then(|t| t.table::<AssemblyRaw>())
            .map(|t| t.get(1))
            .transpose()?
            .flatten()
        else {
            return Ok(None);
        };
        self.scope(
            row.name,
            row.culture,
            [
                row.major_version,
                row.minor_version,
                row.build_number,
                row.revision_number,
            ],
            row.flags,
            row.public_key,
        )
        .map(Some)
    }
    fn scope(
        &mut self,
        name: u32,
        culture: u32,
        version: [u32; 4],
        flags: u32,
        key: u32,
    ) -> Result<ScopeIdentity> {
        let name = self.input.name(name)?.to_owned();
        let culture = self.input.name(culture)?.to_owned();
        let key = if key == 0 {
            Vec::new()
        } else {
            self.input.blob(key)?.to_vec()
        };
        self.work.charge(name.len() + culture.len() + key.len())?;
        // Keep key/full-key flag and all identity flags exact. A full key and
        // public-key token are deliberately not equated without proof.
        Ok(ScopeIdentity::Assembly {
            name,
            culture,
            version,
            flags,
            key,
        })
    }
    fn normalize_method(
        &mut self,
        mut sig: MethodSignature,
        args: Option<&[TypeSignature]>,
        depth: usize,
    ) -> Result<MethodSignature> {
        sig.return_type = self.normalize(sig.return_type, args, depth + 1)?;
        sig.parameters = sig
            .parameters
            .into_iter()
            .map(|ty| self.normalize(ty, args, depth + 1))
            .collect::<Result<_>>()?;
        Ok(sig)
    }
    fn normalize(
        &mut self,
        ty: TypeSignature,
        args: Option<&[TypeSignature]>,
        depth: usize,
    ) -> Result<TypeSignature> {
        ensure!(depth <= 64, "RESOURCE_LIMIT: semantic signature nesting");
        self.work.charge(1)?;
        Ok(match ty {
            TypeSignature::TypeParameter { index } if args.is_some_and(|args| !args.is_empty()) => {
                let argument = args
                    .unwrap()
                    .get(index as usize)
                    .context("INVALID_INPUT: owner generic argument arity")?;
                argument.charge_clone(&mut self.work, depth)?;
                argument.clone()
            }
            TypeSignature::Named { element, token } if token >> 24 == 27 => {
                self.enter(token)?;
                let row = self
                    .input
                    .view
                    .tables()
                    .and_then(|t| t.table::<TypeSpecRaw>())
                    .context("INVALID_INPUT: signature TypeSpec table")?
                    .get(token & 0xffffff)?
                    .context("INVALID_INPUT: signature TypeSpec row")?;
                let signature =
                    signature::ty(self.input, self.input.blob(row.signature)?, &mut self.work)?;
                let signature = self.normalize(signature, args, depth + 1)?;
                self.active.remove(&token);
                if let TypeSignature::Named { token: base, .. }
                | TypeSignature::Constructed { token: base, .. } = &signature
                {
                    let owner = self.owner(*base)?;
                    self.validate_kind(element, &owner)?;
                }
                match &signature {
                    TypeSignature::Named { element: inner, .. }
                    | TypeSignature::Constructed { element: inner, .. }
                        if *inner != element =>
                    {
                        TypeSignature::Specification {
                            element,
                            value: Box::new(signature),
                        }
                    }
                    _ => signature,
                }
            }
            TypeSignature::Named { element, token } => {
                let owner = self.owner(token)?;
                self.validate_kind(element, &owner)?;
                TypeSignature::Named {
                    element,
                    token: owner.token,
                }
            }
            TypeSignature::Constructed {
                element,
                token,
                arguments,
                constructor_arguments,
            } => {
                let owner = self.owner(token)?;
                self.validate_kind(element, &owner)?;
                let mut constructor_arguments = constructor_arguments;
                constructor_arguments.extend(owner.arguments);
                TypeSignature::Constructed {
                    element,
                    token: owner.token,
                    arguments: arguments
                        .into_iter()
                        .map(|arg| self.normalize(arg, args, depth + 1))
                        .collect::<Result<_>>()?,
                    constructor_arguments,
                }
            }
            TypeSignature::Specification { element, value } => TypeSignature::Specification {
                element,
                value: Box::new(self.normalize(*value, args, depth + 1)?),
            },
            TypeSignature::Pointer { value } => TypeSignature::Pointer {
                value: Box::new(self.normalize(*value, args, depth + 1)?),
            },
            TypeSignature::ByRef { value } => TypeSignature::ByRef {
                value: Box::new(self.normalize(*value, args, depth + 1)?),
            },
            TypeSignature::Vector { value } => TypeSignature::Vector {
                value: Box::new(self.normalize(*value, args, depth + 1)?),
            },
            TypeSignature::Pinned { value } => TypeSignature::Pinned {
                value: Box::new(self.normalize(*value, args, depth + 1)?),
            },
            TypeSignature::Array {
                value,
                rank,
                sizes,
                bounds,
            } => TypeSignature::Array {
                value: Box::new(self.normalize(*value, args, depth + 1)?),
                rank,
                sizes,
                bounds,
            },
            TypeSignature::Modified {
                required,
                token,
                mut arguments,
                value,
            } => {
                let owner = self.owner(token)?;
                arguments.extend(owner.arguments);
                TypeSignature::Modified {
                    required,
                    token: owner.token,
                    arguments,
                    value: Box::new(self.normalize(*value, args, depth + 1)?),
                }
            }
            TypeSignature::Function { signature } => TypeSignature::Function {
                signature: Box::new(self.normalize_method(*signature, args, depth + 1)?),
            },
            other => other,
        })
    }
}

// Full keys and public-key tokens need SHA-1 normalization to establish the
// same assembly identity. This backend does not guess that relation by name.
fn unresolved_key_relation(own: &ScopeIdentity, other: &ScopeIdentity) -> bool {
    match (own, other) {
        (
            ScopeIdentity::Assembly {
                name: an,
                version: av,
                culture: ac,
                flags: af,
                key: ak,
            },
            ScopeIdentity::Assembly {
                name: bn,
                version: bv,
                culture: bc,
                flags: bf,
                key: bk,
            },
        ) => {
            an == bn
                && av == bv
                && ac == bc
                && af & !1 == bf & !1
                && af & 1 != bf & 1
                && !ak.is_empty()
                && !bk.is_empty()
        }
        _ => false,
    }
}

pub fn matches(query: &SemanticTarget, candidate: &SemanticTarget, definition: bool) -> bool {
    if query.target_kind == "type" {
        if let Some(canonical) = query.canonical_token {
            return candidate.canonical_token == Some(canonical)
                && (definition
                    || if query.original_token >> 24 == 27 {
                        query.type_signature == candidate.type_signature
                    } else {
                        query.owner_arguments == candidate.owner_arguments
                    });
        }
        return query.original_token == candidate.original_token
            || (query.type_signature == candidate.type_signature
                && (query.original_token >> 24 == 27 || query.scope == candidate.scope));
    }
    if let Some(canonical) = query.canonical_token {
        candidate.canonical_token == Some(canonical)
            && (definition
                || (query.owner_arguments == candidate.owner_arguments
                    && query.method_arguments == candidate.method_arguments))
    } else {
        query.original_token == candidate.original_token
            || (query.owner_token == candidate.owner_token
                && query.scope == candidate.scope
                && query.name == candidate.name
                && query.signature == candidate.signature
                && query.type_signature == candidate.type_signature
                && query.owner_arguments == candidate.owner_arguments
                && query.method_arguments == candidate.method_arguments)
    }
}

pub struct ReferenceResult {
    pub target: Option<Rc<SemanticTarget>>,
    pub references: Vec<Reference>,
    pub work_used: usize,
}

pub struct Reference {
    pub site: crate::TokenSite,
    pub target: Option<Rc<SemanticTarget>>,
}

impl Input {
    pub fn references(&self, token: u32, semantic: bool) -> Result<ReferenceResult> {
        if token >> 24 != 0x70 {
            self.validate_token(token)
                .map_err(|error| anyhow::anyhow!("INVALID_QUERY: {error:#}"))?;
        }
        if semantic {
            ensure!(
                matches!(token >> 24, 1 | 2 | 4 | 6 | 10 | 27 | 43),
                "INVALID_QUERY: semantic metadata target required"
            );
        }
        let scan = self.scan_il()?;
        if token >> 24 == 0x70 {
            ensure!(
                scan.strings
                    .binary_search_by_key(&(token & 0xffffff), |literal| literal.offset)
                    .is_ok(),
                "INVALID_QUERY: token is not a user-string entry"
            );
        }
        let mut resolver = if semantic {
            Some(Resolver::new(self, scan.work_used)?)
        } else {
            None
        };
        let target = resolver
            .as_mut()
            .map(|resolver| resolver.target(token))
            .transpose()?;
        let mut references = Vec::new();
        let mut footprint = 0usize;
        for site in scan.sites {
            let candidate = if let Some(resolver) = &mut resolver {
                if resolver.kind(site.target_token) != resolver.kind(token) {
                    continue;
                }
                Some(resolver.target(site.target_token)?)
            } else {
                None
            };
            let matched = if let Some(target) = &target {
                matches(
                    target,
                    candidate.as_ref().unwrap(),
                    matches!(token >> 24, 2 | 4 | 6),
                )
            } else {
                site.target_token == token
            };
            if !matched {
                continue;
            }
            if let Some(resolver) = &mut resolver {
                resolver.work.charge(1)?;
            }
            ensure!(
                references.len() < self.limits.records,
                "RESOURCE_LIMIT: reference records"
            );
            footprint = footprint
                .checked_add(std::mem::size_of::<Reference>() + site.mnemonic.len())
                .context("RESOURCE_LIMIT: reference memory")?;
            ensure!(
                footprint <= self.limits.output_bytes,
                "RESOURCE_LIMIT: reference memory"
            );
            references.push(Reference {
                site,
                target: candidate,
            });
        }
        Ok(ReferenceResult {
            target,
            references,
            work_used: resolver.map_or(scan.work_used, |resolver| resolver.work.used()),
        })
    }
}
