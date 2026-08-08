use ontolith_query::domain::QueryPlanId;

mod shacl;

pub use shacl::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceMode {
    Off,
    ForwardChaining,
    Hybrid,
}

impl InferenceMode {
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Stable transport/config spelling for the mode (P6-03).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::ForwardChaining => "forward",
            Self::Hybrid => "hybrid",
        }
    }

    /// Parse a config/query-parameter value. Accepts the stable spellings
    /// (`off`/`forward`/`hybrid`) plus common aliases; case-insensitive.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "disabled" | "none" => Some(Self::Off),
            "forward" | "forward-chaining" | "fc" => Some(Self::ForwardChaining),
            "hybrid" => Some(Self::Hybrid),
            _ => None,
        }
    }
}

/// RDFS/OWL RL rule identifiers supported by the forward-chaining engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// rdfs:subClassOf is transitive (RDFS 5).
    SubClassOfTransitive,
    /// rdfs:subPropertyOf is transitive (RDFS 6).
    SubPropertyOfTransitive,
    /// p rdfs:subPropertyOf q ∧ x p y → x q y (RDFS 7 / prp-spo1).
    SubPropertyOf,
    /// p rdfs:domain C ∧ x p y → x rdf:type C (RDFS 7 / prp-dom).
    Domain,
    /// p rdfs:range C ∧ x p y → y rdf:type C (RDFS 8 / prp-rng).
    Range,
    /// p owl:inverseOf q ∧ x p y → y q x (prp-inv1).
    InverseOf,
    /// x rdf:type C ∧ C rdfs:subClassOf D → x rdf:type D (cax-sco).
    SubClassOf,
    /// p rdf:type owl:SymmetricProperty ∧ x p y → y p x (prp-symp).
    SymmetricProperty,
    /// p rdf:type owl:TransitiveProperty ∧ x p y ∧ y p z → x p z (prp-trp).
    TransitiveProperty,
    /// p owl:inverseOf q ∧ x q y → y p x (prp-inv2).
    InverseOfReverse,
    /// x rdf:type (p some C) ∧ x p y → y rdf:type C (cls-svf1).
    SomeValuesFrom,
    /// x p y ∧ y rdf:type C ∧ (p some C) exists → x rdf:type (p some C) (cls-svf2).
    SomeValuesFromTyping,
    /// x rdf:type (p all C) ∧ x p y → y rdf:type C (cls-avf).
    AllValuesFrom,
    /// x rdf:type (C1 ∩ … ∩ Cn) → x rdf:type Ci for every list member (cls-int1).
    IntersectionOf,
    /// x rdf:type Ci for all members of (C1 ∩ … ∩ Cn) → x rdf:type (C1 ∩ … ∩ Cn) (cls-int2).
    IntersectionOfTyping,
    /// x rdf:type Ci ∧ Ci member of (C1 ∪ … ∪ Cn) → x rdf:type (C1 ∪ … ∪ Cn) (cls-uni).
    UnionOf,
    /// x owl:sameAs y → y owl:sameAs x (eq-sym).
    SameAsSymmetric,
    /// x owl:sameAs y ∧ y owl:sameAs z → x owl:sameAs z (eq-trans).
    SameAsTransitive,
    /// ?c owl:hasKey ?u ∧ LIST[?u, ?p1, …, ?pn] ∧ x/y share every key value → x owl:sameAs y (prp-key).
    HasKey,
    /// ?c1 owl:disjointWith ?c2 ∧ x rdf:type ?c1 ∧ x rdf:type ?c2 → ⊥ (cax-dw).
    DisjointClasses,
    /// p rdf:type owl:IrreflexiveProperty ∧ x p x → ⊥ (prp-irp).
    IrreflexiveProperty,
    /// x rdf:type owl:Nothing → ⊥ (cls-nothing1).
    NothingTyping,
    /// ?c rdfs:subClassOf owl:Nothing ∧ x rdf:type ?c → ⊥ (cls-nothing2).
    NothingSubClass,
    /// x owl:sameAs y ∧ x owl:differentFrom y → ⊥ (eq-diff1; x differentFrom x
    /// is the reflexive-sameAs corollary and is detected by the same check).
    SameAsDifferentFrom,
    /// ?x rdf:type owl:AllDifferent ∧ ?x owl:members (?z1 … ?zn) ∧ ?zi owl:sameAs ?zj → ⊥ (eq-diff2).
    AllDifferentMembers,
    /// ?x rdf:type owl:AllDifferent ∧ ?x owl:distinctMembers (?z1 … ?zn) ∧ ?zi owl:sameAs ?zj → ⊥ (eq-diff3).
    AllDifferentDistinctMembers,
    /// ?x rdf:type owl:AllDisjointClasses ∧ ?x owl:members (?c1 … ?cn) ∧ z rdf:type ?ci ∧ z rdf:type ?cj → ⊥ (cax-adc).
    AllDisjointClasses,
    /// ?s owl:sameAs ?s' ∧ ?s ?p ?o → ?s' ?p ?o (eq-rep-s).
    EqualityReplacementSubject,
    /// ?p owl:sameAs ?p' ∧ ?s ?p ?o → ?s ?p' ?o (eq-rep-p).
    EqualityReplacementPredicate,
    /// ?o owl:sameAs ?o' ∧ ?s ?p ?o → ?s ?p ?o' (eq-rep-o).
    EqualityReplacementObject,
    /// p rdf:type owl:FunctionalProperty ∧ x p y1 ∧ x p y2 → y1 owl:sameAs y2 (prp-fp).
    FunctionalProperty,
    /// p rdf:type owl:InverseFunctionalProperty ∧ x1 p y ∧ x2 p y → x1 owl:sameAs x2 (prp-ifp).
    InverseFunctionalProperty,
    /// ?c1 owl:complementOf ?c2 ∧ x rdf:type ?c1 ∧ x rdf:type ?c2 → ⊥ (cls-com).
    ComplementClasses,
    /// x rdf:type (p value y) ∧ restr owl:hasValue y ∧ restr owl:onProperty p → x p y (cls-hv1).
    HasValue,
    /// x p y ∧ restr owl:hasValue y ∧ restr owl:onProperty p → x rdf:type (p value y) (cls-hv2).
    HasValueTyping,
    /// x rdf:type (p max 1) ∧ x p y1 ∧ x p y2 → y1 owl:sameAs y2 (cls-maxc2).
    MaxCardinalityOne,
    /// ?p owl:propertyChainAxiom (?p1 … ?pn) ∧ u1 p1 u2 ∧ … ∧ un pn un+1 → u1 p un+1 (prp-spo2).
    PropertyChain,
}

impl Rule {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SubClassOfTransitive => "rdfs5",
            Self::SubPropertyOfTransitive => "rdfs6",
            Self::SubPropertyOf => "rdfs9",
            Self::Domain => "rdfs7",
            Self::Range => "rdfs8",
            Self::InverseOf => "prp-inv1",
            Self::SubClassOf => "cax-sco",
            Self::SymmetricProperty => "prp-symp",
            Self::TransitiveProperty => "prp-trp",
            Self::InverseOfReverse => "prp-inv2",
            Self::SomeValuesFrom => "cls-svf1",
            Self::SomeValuesFromTyping => "cls-svf2",
            Self::AllValuesFrom => "cls-avf",
            Self::IntersectionOf => "cls-int1",
            Self::IntersectionOfTyping => "cls-int2",
            Self::UnionOf => "cls-uni",
            Self::SameAsSymmetric => "eq-sym",
            Self::SameAsTransitive => "eq-trans",
            Self::HasKey => "prp-key",
            Self::DisjointClasses => "cax-dw",
            Self::IrreflexiveProperty => "prp-irp",
            Self::NothingTyping => "cls-nothing1",
            Self::NothingSubClass => "cls-nothing2",
            Self::SameAsDifferentFrom => "eq-diff1",
            Self::AllDifferentMembers => "eq-diff2",
            Self::AllDifferentDistinctMembers => "eq-diff3",
            Self::AllDisjointClasses => "cax-adc",
            Self::EqualityReplacementSubject => "eq-rep-s",
            Self::EqualityReplacementPredicate => "eq-rep-p",
            Self::EqualityReplacementObject => "eq-rep-o",
            Self::FunctionalProperty => "prp-fp",
            Self::InverseFunctionalProperty => "prp-ifp",
            Self::ComplementClasses => "cls-com",
            Self::HasValue => "cls-hv1",
            Self::HasValueTyping => "cls-hv2",
            Self::MaxCardinalityOne => "cls-maxc2",
            Self::PropertyChain => "prp-spo2",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningTask {
    pub plan_id: Option<QueryPlanId>,
    pub mode: InferenceMode,
    pub max_iterations: u32,
    /// Wall-clock budget per materialization in milliseconds; `None` = unlimited.
    pub max_elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReasoningReport {
    pub inferred_triples: usize,
    pub elapsed_ms: u64,
    /// True when the wall-clock budget was exhausted before convergence.
    pub timed_out: bool,
    /// True when the rule set derived a contradiction (e.g., disjoint classes,
    /// owl:Nothing typing, owl:differentFrom conflicts).
    pub inconsistent: bool,
}

/// Outcome of a materialization run.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterializeOutcome {
    pub triples: Vec<ontolith_rdf::domain::Triple>,
    pub report: ReasoningReport,
}

pub fn status() -> &'static str {
    "domain"
}
