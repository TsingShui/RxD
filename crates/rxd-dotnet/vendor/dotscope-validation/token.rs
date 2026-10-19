// Adapted from dotscope 3028b1ab92ec1b0ad15f046b3b70d5ee82336a00.
// Original: dotscope/src/metadata/validation/validators/raw/structure/token.rs
// Apache-2.0; see LICENSE and NOTICE in this directory.
// Change: cache successful integrity checks of immutable scanner tokens.
// All upstream kind/RID/coded-index/reference checks remain in their order.

use strum::IntoEnumIterator;

use dotscope::{
    Error, Result, dispatch_table_type,
    metadata::{
        tables::{
            CodedIndex, ConstantRaw, CustomAttributeRaw, DeclSecurityRaw, FieldMarshalRaw,
            GenericParamConstraintRaw, GenericParamRaw, InterfaceImplRaw, MemberRefRaw,
            MetadataTable, MethodImplRaw, MethodSpecRaw, NestedClassRaw, TableId, TypeDefRaw,
        },
        token::Token,
        validation::{
            RawValidationContext, RawValidator, ReferenceValidator as UpstreamReferenceValidator,
            TokenValidator, ValidationContext,
        },
    },
};

/// Foundation validator for token format, RID bounds, and coded index validation.
///
/// Ensures the structural integrity of all token references, RID values, and coded indexes
/// across metadata tables. This validator operates at the lowest level of metadata validation,
/// providing essential guarantees before higher-level semantic validation can proceed.
///
/// The validator leverages shared validation utilities (shared token and reference validators)
/// to provide comprehensive coverage
/// of all metadata tables using type-safe dispatch mechanisms and validates both direct token
/// references and encoded coded indexes according to ECMA-335 specifications.
///
/// # Thread Safety
///
/// This validator is [`Send`] and [`Sync`] as all validation operations are read-only
/// and operate on immutable metadata structures.
pub struct RawTokenValidator;

impl RawTokenValidator {
    /// Creates a new token validator instance.
    ///
    /// # Returns
    ///
    /// A new [`RawTokenValidator`] ready for validation operations.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Performs comprehensive cross-table reference validation using shared facilities.
    ///
    /// This method leverages shared reference validation utilities to perform
    /// advanced reference analysis including circular dependency detection and reference integrity.
    /// It extracts actual token references from table data rather than making assumptions.
    ///
    /// # Arguments
    ///
    /// * `context` - Raw validation context containing assembly view and scanner
    ///
    /// # Errors
    ///
    /// Returns validation errors if reference integrity issues are detected.
    fn validate_cross_table_references(context: &RawValidationContext) -> Result<()> {
        let assembly_view = context.assembly_view();
        let token_validator = TokenValidator::new(context.reference_scanner());
        let reference_validator = ReferenceValidator::new(context.reference_scanner());

        let mut referenced_tokens = Vec::new();

        if let Some(tables) = assembly_view.tables() {
            if let Some(table) = tables.table::<TypeDefRaw>() {
                for typedef in table {
                    let typedef = typedef?;
                    if typedef.extends.row != 0 {
                        referenced_tokens.push(typedef.extends.token);
                    }
                }
            }

            if let Some(table) = tables.table::<InterfaceImplRaw>() {
                for interface_impl in table {
                    let interface_impl = interface_impl?;
                    token_validator.validate_table_row(TableId::TypeDef, interface_impl.class)?;
                    referenced_tokens.push(interface_impl.interface.token);
                }
            }

            if let Some(table) = tables.table::<MemberRefRaw>() {
                for memberref in table {
                    let memberref = memberref?;
                    referenced_tokens.push(memberref.class.token);
                }
            }

            if let Some(table) = tables.table::<CustomAttributeRaw>() {
                for attr in table {
                    let attr = attr?;
                    referenced_tokens.push(attr.parent.token);
                    referenced_tokens.push(attr.constructor.token);
                }
            }

            if let Some(table) = tables.table::<NestedClassRaw>() {
                for nested in table {
                    let nested = nested?;
                    token_validator.validate_table_row(TableId::TypeDef, nested.nested_class)?;
                    token_validator.validate_table_row(TableId::TypeDef, nested.enclosing_class)?;
                }
            }

            if let Some(table) = tables.table::<GenericParamRaw>() {
                for genparam in table {
                    let genparam = genparam?;
                    referenced_tokens.push(genparam.owner.token);
                }
            }

            if let Some(table) = tables.table::<MethodSpecRaw>() {
                for methodspec in table {
                    let methodspec = methodspec?;
                    referenced_tokens.push(methodspec.method.token);
                }
            }

            if let Some(table) = tables.table::<GenericParamConstraintRaw>() {
                for constraint in table {
                    let constraint = constraint?;
                    token_validator.validate_table_row(TableId::GenericParam, constraint.owner)?;
                    referenced_tokens.push(constraint.constraint.token);
                }
            }

            if let Some(table) = tables.table::<MethodImplRaw>() {
                for method_impl in table {
                    let method_impl = method_impl?;
                    token_validator.validate_table_row(TableId::TypeDef, method_impl.class)?;
                    referenced_tokens.push(method_impl.method_body.token);
                    referenced_tokens.push(method_impl.method_declaration.token);
                }
            }

            if let Some(table) = tables.table::<ConstantRaw>() {
                for constant in table {
                    let constant = constant?;
                    referenced_tokens.push(constant.parent.token);
                }
            }

            if let Some(table) = tables.table::<FieldMarshalRaw>() {
                for marshal in table {
                    let marshal = marshal?;
                    referenced_tokens.push(marshal.parent.token);
                }
            }

            if let Some(table) = tables.table::<DeclSecurityRaw>() {
                for security in table {
                    let security = security?;
                    referenced_tokens.push(security.parent.token);
                }
            }
        }

        reference_validator.validate_token_references(referenced_tokens)?;

        Ok(())
    }

    /// Validates token references across all metadata tables containing token fields.
    ///
    /// Uses shared validation facilities to systematically check all tables that contain
    /// direct token references. This method leverages shared token and reference validators
    /// for comprehensive validation.
    ///
    /// # Arguments
    ///
    /// * `context` - Raw validation context containing assembly view and scanner
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::InvalidToken`] if any token reference is invalid,
    /// including cases where RID values are zero (when not permitted) or reference
    /// non-existent tables or rows.
    fn validate_token_references(context: &RawValidationContext) -> Result<()> {
        let assembly_view = context.assembly_view();
        let token_validator = TokenValidator::new(context.reference_scanner());
        let reference_validator = ReferenceValidator::new(context.reference_scanner());

        if let Some(tables) = assembly_view.tables() {
            if let Some(table) = tables.table::<InterfaceImplRaw>() {
                Self::validate_interfaceimpl_tokens(table, &token_validator, &reference_validator)?;
            }
            if let Some(table) = tables.table::<MemberRefRaw>() {
                Self::validate_memberref_tokens(table, &token_validator, &reference_validator)?;
            }
            if let Some(table) = tables.table::<CustomAttributeRaw>() {
                Self::validate_customattribute_tokens(
                    table,
                    &token_validator,
                    &reference_validator,
                )?;
            }
            if let Some(table) = tables.table::<NestedClassRaw>() {
                Self::validate_nestedclass_tokens(table, &token_validator, &reference_validator)?;
            }
            if let Some(table) = tables.table::<GenericParamRaw>() {
                Self::validate_genericparam_tokens(table, &token_validator, &reference_validator)?;
            }
            if let Some(table) = tables.table::<MethodSpecRaw>() {
                Self::validate_methodspec_tokens(table, &token_validator, &reference_validator)?;
            }
            if let Some(table) = tables.table::<GenericParamConstraintRaw>() {
                Self::validate_genericparamconstraint_tokens(
                    table,
                    &token_validator,
                    &reference_validator,
                )?;
            }
            if let Some(table) = tables.table::<MethodImplRaw>() {
                Self::validate_methodimpl_tokens(table, &token_validator, &reference_validator)?;
            }
            if let Some(table) = tables.table::<ConstantRaw>() {
                Self::validate_constant_tokens(table, &token_validator, &reference_validator)?;
            }
            if let Some(table) = tables.table::<FieldMarshalRaw>() {
                Self::validate_fieldmarshal_tokens(table, &token_validator, &reference_validator)?;
            }
            if let Some(table) = tables.table::<DeclSecurityRaw>() {
                Self::validate_declsecurity_tokens(table, &token_validator, &reference_validator)?;
            }
        }
        Ok(())
    }

    /// Validates RID bounds for all metadata tables using shared validation facilities.
    ///
    /// Uses the [`crate::dispatch_table_type`] macro combined with shared token validation utilities
    /// to validate RID bounds across all possible metadata table types, ensuring no table exceeds
    /// the 24-bit RID limit mandated by the ECMA-335 specification for token encoding.
    ///
    /// # Arguments
    ///
    /// * `context` - Raw validation context containing assembly view and scanner
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::InvalidToken`] if any table has RID count > 0x00FFFFFF,
    /// which would make token encoding impossible using the standard 32-bit token format.
    fn validate_rid_bounds(context: &RawValidationContext) -> Result<()> {
        let assembly_view = context.assembly_view();
        let token_validator = TokenValidator::new(context.reference_scanner());
        if let Some(tables) = assembly_view.tables() {
            for table_id in TableId::iter() {
                let table_type = table_id as u32;
                dispatch_table_type!(table_id, |RawType| {
                    if let Some(table) = tables.table::<RawType>() {
                        let row_count = table.row_count;
                        if row_count > 0x00FF_FFFF {
                            let token_value = (table_type << 24) | row_count;
                            return Err(Error::InvalidToken {
                                token: Token::new(token_value),
                                message: format!(
                                    "{table_id:?} table RID {row_count} exceeds maximum allowed value (0x00FFFFFF)"
                                ),
                            });
                        }

                        for rid in 1..=row_count {
                            token_validator.validate_table_row(table_id, rid)?;
                        }
                    }
                    Ok(()) as Result<()>
                })?;
            }
        }
        Ok(())
    }

    /// Validates coded indexes across all metadata tables containing coded index fields.
    ///
    /// Uses shared validation facilities to validate that coded indexes use valid tag values
    /// and resolve to appropriate target table types according to ECMA-335 coded index specifications.
    /// This implementation leverages shared token validation utilities for comprehensive
    /// coded index validation.
    ///
    /// # Arguments
    ///
    /// * `context` - Raw validation context containing assembly view and scanner
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::InvalidToken`] if any coded index has invalid
    /// tag values or references inappropriate target table types.
    fn validate_coded_indexes(context: &RawValidationContext) -> Result<()> {
        let assembly_view = context.assembly_view();
        let token_validator = TokenValidator::new(context.reference_scanner());
        let reference_validator = ReferenceValidator::new(context.reference_scanner());

        if let Some(tables) = assembly_view.tables() {
            if let Some(table) = tables.table::<TypeDefRaw>() {
                for typedef in table {
                    let typedef = typedef?;
                    Self::validate_coded_index_field(
                        &typedef.extends,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }

            if let Some(table) = tables.table::<InterfaceImplRaw>() {
                for interface_impl in table {
                    let interface_impl = interface_impl?;
                    Self::validate_coded_index_field(
                        &interface_impl.interface,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }

            if let Some(table) = tables.table::<MemberRefRaw>() {
                for memberref in table {
                    let memberref = memberref?;
                    Self::validate_coded_index_field(
                        &memberref.class,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }

            if let Some(table) = tables.table::<CustomAttributeRaw>() {
                for attr in table {
                    let attr = attr?;
                    Self::validate_coded_index_field(
                        &attr.parent,
                        &token_validator,
                        &reference_validator,
                    )?;
                    Self::validate_coded_index_field(
                        &attr.constructor,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }

            if let Some(table) = tables.table::<GenericParamRaw>() {
                for genparam in table {
                    let genparam = genparam?;
                    Self::validate_coded_index_field(
                        &genparam.owner,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }

            if let Some(table) = tables.table::<MethodSpecRaw>() {
                for methodspec in table {
                    let methodspec = methodspec?;
                    Self::validate_coded_index_field(
                        &methodspec.method,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }

            if let Some(table) = tables.table::<GenericParamConstraintRaw>() {
                for constraint in table {
                    let constraint = constraint?;
                    Self::validate_coded_index_field(
                        &constraint.constraint,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }

            if let Some(table) = tables.table::<ConstantRaw>() {
                for constant in table {
                    let constant = constant?;
                    Self::validate_coded_index_field(
                        &constant.parent,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }

            if let Some(table) = tables.table::<FieldMarshalRaw>() {
                for marshal in table {
                    let marshal = marshal?;
                    Self::validate_coded_index_field(
                        &marshal.parent,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }

            if let Some(table) = tables.table::<DeclSecurityRaw>() {
                for security in table {
                    let security = security?;
                    Self::validate_coded_index_field(
                        &security.parent,
                        &token_validator,
                        &reference_validator,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Validates token references in InterfaceImpl table entries using shared facilities.
    fn validate_interfaceimpl_tokens(
        table: &MetadataTable<InterfaceImplRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for interface_impl in table {
            let interface_impl = interface_impl?;
            token_validator.validate_table_row(TableId::TypeDef, interface_impl.class)?;

            let interface_token = interface_impl.interface.token;
            let allowed_tables = interface_impl.interface.ci_type.tables();
            token_validator.validate_typed_token(interface_token, allowed_tables)?;

            reference_validator.validate_token_integrity(interface_token)?;
        }
        Ok(())
    }

    /// Validates token references in MemberRef table entries using shared facilities.
    fn validate_memberref_tokens(
        table: &MetadataTable<MemberRefRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for memberref in table {
            let memberref = memberref?;
            let class_token = memberref.class.token;
            let allowed_tables = memberref.class.ci_type.tables();
            token_validator.validate_typed_token(class_token, allowed_tables)?;

            reference_validator.validate_token_integrity(class_token)?;
        }
        Ok(())
    }

    /// Validates token references in CustomAttribute table entries using shared facilities.
    fn validate_customattribute_tokens(
        table: &MetadataTable<CustomAttributeRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for attr in table {
            let attr = attr?;
            let parent_token = attr.parent.token;
            token_validator.validate_token_bounds(parent_token)?;
            reference_validator.validate_token_integrity(parent_token)?;

            let constructor_token = attr.constructor.token;
            let allowed_tables = attr.constructor.ci_type.tables();
            token_validator.validate_typed_token(constructor_token, allowed_tables)?;
            reference_validator.validate_token_integrity(constructor_token)?;
        }
        Ok(())
    }

    /// Validates token references in NestedClass table entries using shared facilities.
    fn validate_nestedclass_tokens(
        table: &MetadataTable<NestedClassRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for nested in table {
            let nested = nested?;
            token_validator.validate_table_row(TableId::TypeDef, nested.nested_class)?;

            token_validator.validate_table_row(TableId::TypeDef, nested.enclosing_class)?;

            let nested_class_token =
                Token::new(u32::from(TableId::TypeDef.token_type()) << 24 | nested.nested_class);
            let enclosing_class_token =
                Token::new(u32::from(TableId::TypeDef.token_type()) << 24 | nested.enclosing_class);

            reference_validator
                .validate_nested_class_relationship(enclosing_class_token, nested_class_token)?;
        }
        Ok(())
    }

    /// Validates token references in GenericParam table entries using shared facilities.
    fn validate_genericparam_tokens(
        table: &MetadataTable<GenericParamRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for genparam in table {
            let genparam = genparam?;
            let owner_token = genparam.owner.token;
            let allowed_tables = genparam.owner.ci_type.tables();
            token_validator.validate_typed_token(owner_token, allowed_tables)?;
            reference_validator.validate_token_integrity(owner_token)?;
        }
        Ok(())
    }

    /// Validates token references in MethodSpec table entries.
    fn validate_methodspec_tokens(
        table: &MetadataTable<MethodSpecRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for methodspec in table {
            let methodspec = methodspec?;
            let method_token = methodspec.method.token;
            let allowed_tables = methodspec.method.ci_type.tables();
            token_validator.validate_typed_token(method_token, allowed_tables)?;
            reference_validator.validate_token_integrity(method_token)?;
        }
        Ok(())
    }

    /// Validates token references in GenericParamConstraint table entries.
    fn validate_genericparamconstraint_tokens(
        table: &MetadataTable<GenericParamConstraintRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for constraint in table {
            let constraint = constraint?;
            token_validator.validate_table_row(TableId::GenericParam, constraint.owner)?;

            let constraint_token = constraint.constraint.token;
            let allowed_tables = constraint.constraint.ci_type.tables();
            token_validator.validate_typed_token(constraint_token, allowed_tables)?;
            reference_validator.validate_token_integrity(constraint_token)?;
        }
        Ok(())
    }

    /// Validates token references in MethodImpl table entries.
    fn validate_methodimpl_tokens(
        table: &MetadataTable<MethodImplRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for method_impl in table {
            let method_impl = method_impl?;
            token_validator.validate_table_row(TableId::TypeDef, method_impl.class)?;

            let body_token = method_impl.method_body.token;
            let allowed_tables = method_impl.method_body.ci_type.tables();
            token_validator.validate_typed_token(body_token, allowed_tables)?;
            reference_validator.validate_token_integrity(body_token)?;

            let declaration_token = method_impl.method_declaration.token;
            let allowed_tables = method_impl.method_declaration.ci_type.tables();
            token_validator.validate_typed_token(declaration_token, allowed_tables)?;
            reference_validator.validate_token_integrity(declaration_token)?;
        }
        Ok(())
    }

    /// Validates token references in Constant table entries.
    fn validate_constant_tokens(
        table: &MetadataTable<ConstantRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for constant in table {
            let constant = constant?;
            let parent_token = constant.parent.token;
            token_validator.validate_token_bounds(parent_token)?;
            reference_validator.validate_token_integrity(parent_token)?;
        }
        Ok(())
    }

    /// Validates token references in FieldMarshal table entries.
    fn validate_fieldmarshal_tokens(
        table: &MetadataTable<FieldMarshalRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for marshal in table {
            let marshal = marshal?;
            let parent_token = marshal.parent.token;
            let allowed_tables = marshal.parent.ci_type.tables();
            token_validator.validate_typed_token(parent_token, allowed_tables)?;
            reference_validator.validate_token_integrity(parent_token)?;
        }
        Ok(())
    }

    /// Validates token references in DeclSecurity table entries.
    fn validate_declsecurity_tokens(
        table: &MetadataTable<DeclSecurityRaw>,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        for security in table {
            let security = security?;
            let parent_token = security.parent.token;
            let allowed_tables = security.parent.ci_type.tables();
            token_validator.validate_typed_token(parent_token, allowed_tables)?;
            reference_validator.validate_token_integrity(parent_token)?;
        }
        Ok(())
    }

    /// Generic helper method to validate any coded index field.
    ///
    /// This method replaces the repetitive validation logic found in table-specific methods
    /// by leveraging the CodedIndex's built-in type information to determine valid tables.
    ///
    /// ## Arguments
    ///
    /// * `coded_index` - The coded index to validate
    /// * `token_validator` - Token validator for checking token format and bounds
    /// * `reference_validator` - Reference validator for checking token integrity
    ///
    /// ## Returns
    ///
    /// Returns `Ok(())` if validation passes, or an error if validation fails.
    fn validate_coded_index_field(
        coded_index: &CodedIndex,
        token_validator: &TokenValidator,
        reference_validator: &ReferenceValidator,
    ) -> Result<()> {
        if coded_index.row != 0 {
            let token = coded_index.token;
            let allowed_tables = coded_index.ci_type.tables();
            token_validator.validate_typed_token(token, allowed_tables)?;
            reference_validator.validate_token_integrity(token)?;
        }
        Ok(())
    }
}

impl RawValidator for RawTokenValidator {
    fn validate_raw(&self, context: &RawValidationContext) -> Result<()> {
        Self::validate_token_references(context)?;
        Self::validate_rid_bounds(context)?;
        Self::validate_coded_indexes(context)?;

        if context.config().enable_cross_table_validation {
            Self::validate_cross_table_references(context)?;
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "RawTokenValidator"
    }

    fn priority(&self) -> u32 {
        200
    }

    fn should_run(&self, context: &RawValidationContext) -> bool {
        context.config().enable_structural_validation
    }
}

impl Default for RawTokenValidator {
    fn default() -> Self {
        Self::new()
    }
}

// This checker is created per validation phase and never outlives the immutable
// scanner. Every unique target executes the full upstream integrity check;
// failures are never cached. Type/kind/coded-index checks remain unconditional.
struct ReferenceValidator<'a> {
    upstream: UpstreamReferenceValidator<'a>,
    checked: std::cell::RefCell<std::collections::HashSet<Token>>,
}
impl<'a> ReferenceValidator<'a> {
    fn new(scanner: &'a dotscope::metadata::validation::ReferenceScanner) -> Self {
        Self {
            upstream: UpstreamReferenceValidator::new(scanner),
            checked: Default::default(),
        }
    }
    fn validate_token_integrity(&self, token: Token) -> Result<()> {
        if self.checked.borrow().contains(&token) {
            return Ok(());
        }
        self.upstream.validate_token_integrity(token)?;
        self.checked.borrow_mut().insert(token);
        Ok(())
    }
    fn validate_token_references<I: IntoIterator<Item = Token>>(&self, tokens: I) -> Result<()> {
        self.upstream.validate_token_references(tokens)
    }
}
impl<'a> std::ops::Deref for ReferenceValidator<'a> {
    type Target = UpstreamReferenceValidator<'a>;
    fn deref(&self) -> &Self::Target {
        &self.upstream
    }
}
