//! The shared `xref` target surface.
//!
//! Every backend that answers "who references this?" takes the same two faces, so the
//! vocabulary and its error messages live here instead of once per backend:
//!
//! - `--at COORD` names one identity, exactly as a row prints it. The coordinate says what
//!   kind it is (`Lx/y;` a type, `Lx/y;->m()V` a method), so `--kind` is only needed to read
//!   `--at` as a literal, which is the one target that cannot describe itself.
//! - `-m/--match PATTERN` searches content: a case-sensitive substring, where an empty
//!   answer is a true answer rather than a failure. A content search cannot say what kind it
//!   looks for, so `--kind` is required here.
//!
//! A coordinate's kind is not negotiable. `--at 'La/a;->m()V' --kind field` is an error that
//! names both kinds, never an empty result set: a silently empty answer reads exactly like a
//! true one, which is the failure mode both faces exist to prevent.

#[cfg(any(feature = "backend-dex", feature = "backend-jvm"))]
use super::filter::CaseMode;
use anyhow::{Context, Result, bail};
use clap::Args;
#[cfg(any(
    feature = "backend-dex",
    feature = "backend-jvm",
    feature = "backend-dotnet"
))]
use clap::ValueEnum;

/// Which kind of target a reference query names.
#[cfg(any(
    feature = "backend-dex",
    feature = "backend-jvm",
    feature = "backend-dotnet"
))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Kind {
    String,
    Type,
    Method,
    Field,
}

#[cfg(any(
    feature = "backend-dex",
    feature = "backend-jvm",
    feature = "backend-dotnet"
))]
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Type => "type",
            Self::Method => "method",
            Self::Field => "field",
        }
    }
}

/// One identity, as a row prints it.
#[cfg(any(feature = "backend-dex", feature = "backend-jvm"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// A string literal. It has no coordinate of its own, so `--kind string` names it.
    String(String),
    /// A type: `Lx/y;`, `[Lx/y;`, or a dotted `x.y`.
    Type(String),
    Method {
        class: String,
        name: String,
        /// The JVM/DEX prototype, e.g. `(ILjava/lang/String;)V`.
        descriptor: String,
    },
    Field {
        class: String,
        name: String,
        /// The field's type descriptor, e.g. `Ljava/lang/String;`.
        descriptor: String,
    },
}

#[cfg(any(feature = "backend-dex", feature = "backend-jvm"))]
impl Anchor {
    pub fn kind(&self) -> Kind {
        match self {
            Self::String(_) => Kind::String,
            Self::Type(_) => Kind::Type,
            Self::Method { .. } => Kind::Method,
            Self::Field { .. } => Kind::Field,
        }
    }

    /// The coordinate as the backend's own `get-*` commands spell it, for error messages.
    pub fn text(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Type(name) => name.clone(),
            Self::Method {
                class,
                name,
                descriptor,
            } => format!("{class}->{name}{descriptor}"),
            Self::Field {
                class,
                name,
                descriptor,
            } => format!("{class}->{name}:{descriptor}"),
        }
    }
}

/// What a resolved target query asks for.
#[cfg(any(feature = "backend-dex", feature = "backend-jvm"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub kind: Kind,
    pub anchor: Option<Anchor>,
    /// The content search pattern, for the `-f` face.
    pub filter: Option<String>,
    /// An owner class constraint: one exact class, never a pattern. A member search that
    /// wants "owners whose name contains X" lists `classes -f X` first.
    pub class: Option<String>,
    /// Whether `filter` folds case. Coordinates and `--class` are matched exactly.
    pub case_sensitive: bool,
}

#[cfg(any(feature = "backend-dex", feature = "backend-jvm"))]
impl Target {
    /// The header's echo of the query. Only fields the query actually used are present.
    pub fn json(&self) -> serde_json::Value {
        let mut query = serde_json::Map::new();
        query.insert("kind".into(), self.kind.as_str().into());
        if let Some(anchor) = self.anchor.as_ref().map(Anchor::text) {
            query.insert("at".into(), anchor.into());
        }
        if let Some(filter) = &self.filter {
            query.insert("match".into(), filter.clone().into());
            query.insert(
                "case".into(),
                (if self.case_sensitive {
                    "sensitive"
                } else {
                    "insensitive"
                })
                .into(),
            );
        }
        if let Some(class) = &self.class {
            query.insert("class".into(), class.clone().into());
        }
        serde_json::Value::Object(query)
    }
}

/// One FETCH selector: the coordinate a row printed.
#[derive(Debug, Args)]
pub struct Selector {
    /// Coordinate copied from a record, exactly as that row printed it.
    ///
    /// Not marked required by clap: a command may offer another way in (`--byte`, a
    /// content search), and `resolve` then reports the missing selector itself.
    #[arg(long, value_name = "COORD")]
    pub at: Option<String>,
}

impl Selector {
    /// Resolve the coordinate to fetch.
    pub fn resolve(&self) -> anyhow::Result<String> {
        self.at.clone().ok_or_else(|| {
            rxd_core::Error::failure(rxd_core::Failure::Query, "--at <COORD> is required").into()
        })
    }
}

/// The two faces every backend shares.
#[cfg(any(feature = "backend-dex", feature = "backend-jvm"))]
#[derive(Debug, Args)]
pub struct TargetArgs {
    /// Exact target coordinate, as a row prints it: `Lx/y;`, `Lx/y;->name(proto)`,
    /// `Lx/y;->name:type`, or, with `--kind string`, a literal.
    #[arg(long, value_name = "COORD", conflicts_with = "filter")]
    pub at: Option<String>,
    /// Substring of the target's value, name or type; folded unless `--case sensitive`.
    #[arg(short = 'm', long = "match", value_name = "PATTERN")]
    pub filter: Option<String>,
    /// Target kind: inferred from `--at` when the coordinate carries one, required with
    /// `--match`, and required to read `--at` as a literal.
    #[arg(long, value_enum)]
    pub kind: Option<Kind>,
    /// Owner class, one exact class: `Lpkg/Name;`, `pkg/Name` or `pkg.Name`. A member
    /// query only; a pattern over owners is `classes -f PATTERN` first.
    #[arg(long = "class", value_name = "CLASS")]
    pub class_name: Option<String>,
    /// Whether matching folds case: `insensitive` (default) or `sensitive`. A coordinate's
    /// own identity and `--class` are always exact; this governs only `--match`.
    #[arg(long, value_enum, default_value_t = CaseMode::Insensitive)]
    pub case: CaseMode,
}

#[cfg(any(feature = "backend-dex", feature = "backend-jvm"))]
impl TargetArgs {
    pub fn resolve(&self) -> Result<Target> {
        // Clap rejects `--at` with `--match`; keep the check for direct callers.
        if self.at.is_some() && self.filter.is_some() {
            bail!(
                "--at names one identity and --match searches content; give one of them, \
                 not both"
            );
        }
        match (&self.at, &self.filter) {
            (Some(coordinate), None) => self.anchor(coordinate),
            (None, Some(pattern)) => self.search(pattern),
            (None, None) => bail!(
                "xref needs --at COORD to take references to one identity, or --match \
                 PATTERN --kind KIND to search them by content"
            ),
            (Some(_), Some(_)) => unreachable!("rejected above"),
        }
    }

    /// A coordinate anchor: one identity, matched exactly.
    fn anchor(&self, coordinate: &str) -> Result<Target> {
        let coordinate = coordinate.trim();
        if coordinate.is_empty() {
            bail!("--at is empty");
        }
        // A literal is the one target that cannot describe itself, so it is the one place
        // `--kind` is required to read `--at`.
        if self.kind == Some(Kind::String) {
            return Ok(Target {
                kind: Kind::String,
                anchor: Some(Anchor::String(coordinate.to_owned())),
                filter: None,
                class: None,
                case_sensitive: self.case.is_sensitive(),
            });
        }
        let anchor = if coordinate.contains("->") {
            let (class, rest) = coordinate
                .split_once("->")
                .expect("checked for the separator above");
            match rest.find('(') {
                Some(open) => {
                    let (name, descriptor) = rest.split_at(open);
                    // The prototype is what selects one overload: `m()V` and `m(I)V` are
                    // different methods, so a coordinate without one would be ambiguous.
                    // A prototype carries its return type after the `)`, never a bare `)`.
                    if !descriptor.contains(')') {
                        bail!(
                            "{coordinate} is not 'Lclass;->name(proto)': the prototype has no \
                             closing ')'"
                        );
                    }
                    if name.is_empty() {
                        bail!("{coordinate} has no member name");
                    }
                    Anchor::Method {
                        class: class.to_owned(),
                        name: name.to_owned(),
                        descriptor: descriptor.to_owned(),
                    }
                }
                None => {
                    let (name, descriptor) = rest.split_once(':').with_context(|| {
                        format!(
                            "{coordinate} is not 'Lclass;->name(type)': a field coordinate \
                             carries its type"
                        )
                    })?;
                    if name.is_empty() {
                        bail!("{coordinate} has no member name");
                    }
                    Anchor::Field {
                        class: class.to_owned(),
                        name: name.to_owned(),
                        descriptor: descriptor.to_owned(),
                    }
                }
            }
        } else {
            Anchor::Type(coordinate.to_owned())
        };
        let kind = anchor.kind();
        if let Some(given) = self.kind
            && given != kind
        {
            bail!(
                "--at {coordinate} is a {} coordinate, but --kind {} was given",
                kind.as_str(),
                given.as_str()
            );
        }
        if self.class_name.is_some() {
            bail!(
                "--at already names the owner class; --class would narrow a coordinate that \
                 is already exact"
            );
        }
        Ok(Target {
            kind,
            anchor: Some(anchor),
            filter: None,
            class: None,
            case_sensitive: self.case.is_sensitive(),
        })
    }

    /// A content search: a case-sensitive substring of the target's value, name or type.
    fn search(&self, pattern: &str) -> Result<Target> {
        let Some(kind) = self.kind else {
            bail!("--match searches content, so it needs --kind: string, type, method or field");
        };
        // An empty pattern is the substring every value contains, matching the listing
        // commands' empty `--filter`: no restriction.
        let class = self
            .class_name
            .as_deref()
            .filter(|class| !class.is_empty())
            .map(str::to_owned);
        match kind {
            Kind::String | Kind::Type if class.is_some() => bail!(
                "--class narrows a member's owner class; it needs --kind method or --kind \
                 field, not --kind {}",
                kind.as_str()
            ),
            _ => {}
        }
        Ok(Target {
            kind,
            anchor: None,
            filter: Some(pattern.to_owned()),
            class,
            case_sensitive: self.case.is_sensitive(),
        })
    }
}

#[cfg(all(test, any(feature = "backend-dex", feature = "backend-jvm")))]
mod tests {
    use super::*;

    fn args(at: Option<&str>, filter: Option<&str>, kind: Option<Kind>) -> TargetArgs {
        TargetArgs {
            at: at.map(str::to_owned),
            filter: filter.map(str::to_owned),
            kind,
            class_name: None,
            case: CaseMode::Insensitive,
        }
    }

    #[test]
    fn a_coordinate_says_its_own_kind() {
        assert_eq!(
            args(Some("La/a;->m(I)V"), None, None).resolve().unwrap(),
            Target {
                kind: Kind::Method,
                anchor: Some(Anchor::Method {
                    class: "La/a;".into(),
                    name: "m".into(),
                    descriptor: "(I)V".into(),
                }),
                filter: None,
                class: None,
                case_sensitive: false,
            }
        );
        assert_eq!(
            args(Some("La/a;->f:I"), None, None).resolve().unwrap().kind,
            Kind::Field
        );
        assert_eq!(
            args(Some("La/a;"), None, None).resolve().unwrap().kind,
            Kind::Type
        );
    }

    #[test]
    fn a_contradicting_kind_names_both() {
        let error = args(Some("La/a;->m()V"), None, Some(Kind::Field))
            .resolve()
            .unwrap_err()
            .to_string();
        assert!(error.contains("is a method coordinate"), "{error}");
        assert!(error.contains("--kind field was given"), "{error}");
    }

    #[test]
    fn a_match_needs_a_kind() {
        let error = args(None, Some("m"), None)
            .resolve()
            .unwrap_err()
            .to_string();
        assert!(error.contains("needs --kind"), "{error}");
    }

    #[test]
    fn the_two_faces_are_exclusive() {
        let error = args(Some("La/a;"), Some("m"), None)
            .resolve()
            .unwrap_err()
            .to_string();
        assert!(error.contains("give one of them, not both"), "{error}");
    }
}
