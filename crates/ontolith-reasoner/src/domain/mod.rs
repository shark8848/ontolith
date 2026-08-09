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
    // Table 4 — Equality (eq-*).
    /// ?s ?p ?o → ?s owl:sameAs ?s, ?p owl:sameAs ?p, ?o owl:sameAs ?o (eq-ref).
    /// Reflexivity is a tautology; the engine materializes it for every term
    /// already connected through an owl:sameAs edge (eq-sym + eq-trans derive
    /// the same loops, so sameAs components are reflexive without global blow-up).
    EqRef,
    /// x owl:sameAs y → y owl:sameAs x (eq-sym).
    SameAsSymmetric,
    /// x owl:sameAs y ∧ y owl:sameAs z → x owl:sameAs z (eq-trans).
    SameAsTransitive,
    /// s owl:sameAs s' ∧ s p o → s' p o (eq-rep-s).
    EqualityReplacementSubject,
    /// p owl:sameAs p' ∧ s p o → s p' o (eq-rep-p).
    EqualityReplacementPredicate,
    /// o owl:sameAs o' ∧ s p o → s p o' (eq-rep-o).
    EqualityReplacementObject,
    /// x owl:sameAs y ∧ x owl:differentFrom y → ⊥ (eq-diff1; the reflexive
    /// corollary x differentFrom x is detected by the same check).
    SameAsDifferentFrom,
    /// ?x rdf:type owl:AllDifferent ∧ ?x owl:members (?z1 … ?zn) ∧ ?zi owl:sameAs ?zj → ⊥ (eq-diff2).
    AllDifferentMembers,
    /// ?x rdf:type owl:AllDifferent ∧ ?x owl:distinctMembers (?z1 … ?zn) ∧ ?zi owl:sameAs ?zj → ⊥ (eq-diff3).
    AllDifferentDistinctMembers,

    // Table 5 — Property axioms (prp-*).
    /// The built-in annotation properties are rdf:type owl:AnnotationProperty (prp-ap, axiomatic).
    AnnotationProperty,
    /// p rdfs:domain C ∧ x p y → x rdf:type C (prp-dom).
    Domain,
    /// p rdfs:range C ∧ x p y → y rdf:type C (prp-rng).
    Range,
    /// p rdf:type owl:FunctionalProperty ∧ x p y1 ∧ x p y2 → y1 owl:sameAs y2 (prp-fp).
    FunctionalProperty,
    /// p rdf:type owl:InverseFunctionalProperty ∧ x1 p y ∧ x2 p y → x1 owl:sameAs x2 (prp-ifp).
    InverseFunctionalProperty,
    /// p rdf:type owl:IrreflexiveProperty ∧ x p x → ⊥ (prp-irp).
    IrreflexiveProperty,
    /// p rdf:type owl:SymmetricProperty ∧ x p y → y p x (prp-symp).
    SymmetricProperty,
    /// p rdf:type owl:AsymmetricProperty ∧ x p y ∧ y p x → ⊥ (prp-asyp).
    AsymmetricProperty,
    /// p rdf:type owl:TransitiveProperty ∧ x p y ∧ y p z → x p z (prp-trp).
    TransitiveProperty,
    /// p rdfs:subPropertyOf q ∧ x p y → x q y (prp-spo1).
    SubPropertyOf,
    /// ?p owl:propertyChainAxiom (?p1 … ?pn) ∧ u1 p1 u2 ∧ … ∧ un pn un+1 → u1 p un+1 (prp-spo2).
    PropertyChain,
    /// p1 owl:equivalentProperty p2 → p1 rdfs:subPropertyOf p2 (prp-eqp1).
    EquivalentProperty,
    /// p1 owl:equivalentProperty p2 → p2 rdfs:subPropertyOf p1 (prp-eqp2).
    EquivalentPropertyReverse,
    /// p1 owl:propertyDisjointWith p2 ∧ x p1 y ∧ x p2 y → ⊥ (prp-pdw).
    PropertyDisjointWith,
    /// ?x rdf:type owl:AllDisjointProperties ∧ ?x owl:members (?p1 … ?pn) ∧ u pi v ∧ u pj v → ⊥ (prp-adp).
    AllDisjointProperties,
    /// p owl:inverseOf q ∧ x p y → y q x (prp-inv1).
    InverseOf,
    /// p owl:inverseOf q ∧ x q y → y p x (prp-inv2).
    InverseOfReverse,
    /// ?c owl:hasKey ?u ∧ LIST[?u, ?p1, …, ?pn] ∧ x/y share every key value → x owl:sameAs y (prp-key).
    HasKey,
    /// ?n owl:sourceIndividual i ∧ owl:assertionProperty p ∧ owl:targetIndividual j ∧ i p j → ⊥ (prp-npa1).
    NegativePropertyAssertionObject,
    /// ?n owl:sourceIndividual i ∧ owl:assertionProperty p ∧ owl:targetValue v ∧ i p v → ⊥ (prp-npa2).
    NegativePropertyAssertionValue,

    // Table 6 — Class expressions (cls-*).
    /// owl:Thing rdf:type owl:Class (cls-thing, axiomatic).
    ThingClass,
    /// owl:Nothing rdf:type owl:Class (cls-nothing1, axiomatic).
    NothingClass,
    /// x rdf:type owl:Nothing → ⊥ (cls-nothing2).
    NothingTyping,
    /// ?c rdfs:subClassOf owl:Nothing ∧ x rdf:type ?c → ⊥ (cls-nothing3; sound
    /// derived extension of cax-sco + cls-nothing2, kept as a direct rule for
    /// single-iteration detection).
    NothingSubClass,
    /// x rdf:type (C1 ∩ … ∩ Cn) → x rdf:type Ci for every list member (cls-int1).
    IntersectionOf,
    /// x rdf:type Ci for all members of (C1 ∩ … ∩ Cn) → x rdf:type (C1 ∩ … ∩ Cn) (cls-int2).
    IntersectionOfTyping,
    /// x rdf:type Ci ∧ Ci member of (C1 ∪ … ∪ Cn) → x rdf:type (C1 ∪ … ∪ Cn) (cls-uni).
    UnionOf,
    /// x rdf:type C1 ∧ x rdf:type C2 ∧ C1 owl:complementOf C2 → ⊥ (cls-com).
    ComplementClasses,
    /// x rdf:type (p some C) ∧ x p y → y rdf:type C (cls-svf1).
    SomeValuesFrom,
    /// x p y ∧ y rdf:type C ∧ (p some C) exists → x rdf:type (p some C) (cls-svf2).
    SomeValuesFromTyping,
    /// x rdf:type (p all C) ∧ x p y → y rdf:type C (cls-avf).
    AllValuesFrom,
    /// x rdf:type (p value y) ∧ restr owl:onProperty p ∧ restr owl:hasValue y → x p y (cls-hv1).
    HasValue,
    /// x p y ∧ restr owl:onProperty p ∧ restr owl:hasValue y → x rdf:type (p value y) (cls-hv2).
    HasValueTyping,
    /// (p max 0) ∧ u rdf:type (p max 0) ∧ u p y → ⊥ (cls-maxc1).
    MaxCardinalityZero,
    /// x rdf:type (p max 1) ∧ x p y1 ∧ x p y2 → y1 owl:sameAs y2 (cls-maxc2).
    MaxCardinalityOne,
    /// (p max 0 c) ∧ u rdf:type (p max 0 c) ∧ u p y ∧ y rdf:type c → ⊥ (cls-maxqc1).
    MaxQualifiedCardinalityZero,
    /// (p max 0 owl:Thing) ∧ u rdf:type (p max 0 owl:Thing) ∧ u p y → ⊥ (cls-maxqc2).
    MaxQualifiedCardinalityZeroThing,
    /// (p max 1 c) ∧ u rdf:type (p max 1 c) ∧ u p y1/y2 ∧ y1/y2 rdf:type c → y1 owl:sameAs y2 (cls-maxqc3).
    MaxQualifiedCardinalityOne,
    /// (p max 1 owl:Thing) ∧ u rdf:type (p max 1 owl:Thing) ∧ u p y1/y2 → y1 owl:sameAs y2 (cls-maxqc4).
    MaxQualifiedCardinalityOneThing,
    /// ?c owl:oneOf (?y1 … ?yn) → ?yi rdf:type ?c for every member (cls-oo).
    OneOf,

    // Table 7 — Class axioms (cax-*).
    /// x rdf:type C ∧ C rdfs:subClassOf D → x rdf:type D (cax-sco).
    SubClassOf,
    /// C1 owl:equivalentClass C2 ∧ x rdf:type C1 → x rdf:type C2 (cax-eqc1).
    EquivalentClass,
    /// C1 owl:equivalentClass C2 ∧ x rdf:type C2 → x rdf:type C1 (cax-eqc2).
    EquivalentClassReverse,
    /// C1 owl:disjointWith C2 ∧ x rdf:type C1 ∧ x rdf:type C2 → ⊥ (cax-dw).
    DisjointClasses,
    /// ?x rdf:type owl:AllDisjointClasses ∧ ?x owl:members (?c1 … ?cn) ∧ z rdf:type ci ∧ z rdf:type cj → ⊥ (cax-adc).
    AllDisjointClasses,

    // Table 8 — Datatypes (dt-*).
    /// Supported datatypes are rdf:type rdfs:Datatype (dt-type1, axiomatic).
    DatatypeTyping,
    /// Literal lt with supported datatype dt → lt rdf:type dt (dt-type2; literal
    /// subjects are not representable in the storage model, so the engine enforces
    /// the value space through dt-not-type and dt-eq instead).
    DatatypeLiteralTyping,
    /// A literal whose lexical form lies outside its datatype's value space → ⊥ (dt-not-type).
    DatatypeNotType,
    /// Literals with the same data value are owl:sameAs / interchangeable as
    /// triple objects (dt-eq; bounded to the graph's literals).
    DatatypeEquality,
    /// Literals with different data values are owl:differentFrom (dt-diff; like
    /// the reference implementation this is not materialized pairwise).
    DatatypeDifference,

    // Table 9 — Schema vocabulary (scm-*).
    /// ?c rdf:type owl:Class → ?c rdfs:subClassOf ?c, ?c owl:equivalentClass ?c,
    /// ?c rdfs:subClassOf owl:Thing, owl:Nothing rdfs:subClassOf ?c (scm-cls).
    ClassSchema,
    /// C1 rdfs:subClassOf C2 ∧ C2 rdfs:subClassOf C3 → C1 rdfs:subClassOf C3 (scm-sco).
    SubClassOfTransitive,
    /// C1 owl:equivalentClass C2 → C1 rdfs:subClassOf C2 ∧ C2 rdfs:subClassOf C1 (scm-eqc1).
    EquivalentClassSchema,
    /// C1 rdfs:subClassOf C2 ∧ C2 rdfs:subClassOf C1 → C1 owl:equivalentClass C2 (scm-eqc2).
    EquivalentClassSchemaReverse,
    /// ?p rdf:type owl:ObjectProperty → ?p rdfs:subPropertyOf ?p ∧ ?p owl:equivalentProperty ?p (scm-op).
    ObjectPropertySchema,
    /// ?p rdf:type owl:DatatypeProperty → ?p rdfs:subPropertyOf ?p ∧ ?p owl:equivalentProperty ?p (scm-dp).
    DatatypePropertySchema,
    /// P1 rdfs:subPropertyOf P2 ∧ P2 rdfs:subPropertyOf P3 → P1 rdfs:subPropertyOf P3 (scm-spo).
    SubPropertyOfTransitive,
    /// P1 owl:equivalentProperty P2 → P1 rdfs:subPropertyOf P2 ∧ P2 rdfs:subPropertyOf P1 (scm-eqp1).
    EquivalentPropertySchema,
    /// P1 rdfs:subPropertyOf P2 ∧ P2 rdfs:subPropertyOf P1 → P1 owl:equivalentProperty P2 (scm-eqp2).
    EquivalentPropertySchemaReverse,
    /// p rdfs:domain C1 ∧ C1 rdfs:subClassOf C2 → p rdfs:domain C2 (scm-dom1).
    DomainSchema,
    /// P2 rdfs:domain C ∧ P1 rdfs:subPropertyOf P2 → P1 rdfs:domain C (scm-dom2).
    DomainSchemaSubproperty,
    /// p rdfs:range C1 ∧ C1 rdfs:subClassOf C2 → p rdfs:range C2 (scm-rng1).
    RangeSchema,
    /// P2 rdfs:range C ∧ P1 rdfs:subPropertyOf P2 → P1 rdfs:range C (scm-rng2).
    RangeSchemaSubproperty,
    /// C1 owl:hasValue i ∧ owl:onProperty p1 ∧ C2 owl:hasValue i ∧ owl:onProperty p2
    /// ∧ p1 rdfs:subPropertyOf p2 → C1 rdfs:subClassOf C2 (scm-hv).
    HasValueSchema,
    /// C1 owl:someValuesFrom y1 ∧ owl:onProperty p ∧ C2 owl:someValuesFrom y2 ∧ owl:onProperty p
    /// ∧ y1 rdfs:subClassOf y2 → C1 rdfs:subClassOf C2 (scm-svf1).
    SomeValuesSchema,
    /// C1 owl:someValuesFrom y ∧ owl:onProperty p1 ∧ C2 owl:someValuesFrom y ∧ owl:onProperty p2
    /// ∧ p1 rdfs:subPropertyOf p2 → C1 rdfs:subClassOf C2 (scm-svf2).
    SomeValuesSchemaSubproperty,
    /// C1 owl:allValuesFrom y1 ∧ owl:onProperty p ∧ C2 owl:allValuesFrom y2 ∧ owl:onProperty p
    /// ∧ y1 rdfs:subClassOf y2 → C1 rdfs:subClassOf C2 (scm-avf1).
    AllValuesSchema,
    /// C1 owl:allValuesFrom y ∧ owl:onProperty p1 ∧ C2 owl:allValuesFrom y ∧ owl:onProperty p2
    /// ∧ p1 rdfs:subPropertyOf p2 → C2 rdfs:subClassOf C1 (scm-avf2).
    AllValuesSchemaSubproperty,
    /// c owl:intersectionOf (?c1 … ?cn) → c rdfs:subClassOf ci for every member (scm-int).
    IntersectionSchema,
    /// c owl:unionOf (?c1 … ?cn) → ci rdfs:subClassOf c for every member (scm-uni).
    UnionSchema,

    // RDF 1.1 Semantics — RDFS rules not subsumed by OWL 2 RL (rdfs-*).
    /// Datatype-map IRIs are rdf:type rdfs:Datatype (rdfs1; same conclusion as dt-type1).
    DatatypeIriTyping,
    /// s p o → s rdf:type rdfs:Resource (rdfs4a).
    SubjectResource,
    /// s p o → o rdf:type rdfs:Resource (rdfs4b).
    ObjectResource,
    /// p rdf:type rdf:Property → p rdfs:subPropertyOf p (rdfs6).
    PropertyReflexive,
    /// c rdf:type rdfs:Class → c rdfs:subClassOf rdfs:Resource (rdfs8).
    ClassResource,
    /// c rdf:type rdfs:Class → c rdfs:subClassOf c (rdfs10).
    ClassReflexive,
    /// p rdf:type rdfs:ContainerMembershipProperty → p rdfs:subPropertyOf rdfs:member (rdfs12).
    ContainerMembership,
    /// d rdf:type rdfs:Datatype → d rdfs:subClassOf rdfs:Literal (rdfs13).
    DatatypeLiteral,
}

impl Rule {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EqRef => "eq-ref",
            Self::SameAsSymmetric => "eq-sym",
            Self::SameAsTransitive => "eq-trans",
            Self::EqualityReplacementSubject => "eq-rep-s",
            Self::EqualityReplacementPredicate => "eq-rep-p",
            Self::EqualityReplacementObject => "eq-rep-o",
            Self::SameAsDifferentFrom => "eq-diff1",
            Self::AllDifferentMembers => "eq-diff2",
            Self::AllDifferentDistinctMembers => "eq-diff3",
            Self::AnnotationProperty => "prp-ap",
            Self::Domain => "prp-dom",
            Self::Range => "prp-rng",
            Self::FunctionalProperty => "prp-fp",
            Self::InverseFunctionalProperty => "prp-ifp",
            Self::IrreflexiveProperty => "prp-irp",
            Self::SymmetricProperty => "prp-symp",
            Self::AsymmetricProperty => "prp-asyp",
            Self::TransitiveProperty => "prp-trp",
            Self::SubPropertyOf => "prp-spo1",
            Self::PropertyChain => "prp-spo2",
            Self::EquivalentProperty => "prp-eqp1",
            Self::EquivalentPropertyReverse => "prp-eqp2",
            Self::PropertyDisjointWith => "prp-pdw",
            Self::AllDisjointProperties => "prp-adp",
            Self::InverseOf => "prp-inv1",
            Self::InverseOfReverse => "prp-inv2",
            Self::HasKey => "prp-key",
            Self::NegativePropertyAssertionObject => "prp-npa1",
            Self::NegativePropertyAssertionValue => "prp-npa2",
            Self::ThingClass => "cls-thing",
            Self::NothingClass => "cls-nothing1",
            Self::NothingTyping => "cls-nothing2",
            Self::NothingSubClass => "cls-nothing3",
            Self::IntersectionOf => "cls-int1",
            Self::IntersectionOfTyping => "cls-int2",
            Self::UnionOf => "cls-uni",
            Self::ComplementClasses => "cls-com",
            Self::SomeValuesFrom => "cls-svf1",
            Self::SomeValuesFromTyping => "cls-svf2",
            Self::AllValuesFrom => "cls-avf",
            Self::HasValue => "cls-hv1",
            Self::HasValueTyping => "cls-hv2",
            Self::MaxCardinalityZero => "cls-maxc1",
            Self::MaxCardinalityOne => "cls-maxc2",
            Self::MaxQualifiedCardinalityZero => "cls-maxqc1",
            Self::MaxQualifiedCardinalityZeroThing => "cls-maxqc2",
            Self::MaxQualifiedCardinalityOne => "cls-maxqc3",
            Self::MaxQualifiedCardinalityOneThing => "cls-maxqc4",
            Self::OneOf => "cls-oo",
            Self::SubClassOf => "cax-sco",
            Self::EquivalentClass => "cax-eqc1",
            Self::EquivalentClassReverse => "cax-eqc2",
            Self::DisjointClasses => "cax-dw",
            Self::AllDisjointClasses => "cax-adc",
            Self::DatatypeTyping => "dt-type1",
            Self::DatatypeLiteralTyping => "dt-type2",
            Self::DatatypeNotType => "dt-not-type",
            Self::DatatypeEquality => "dt-eq",
            Self::DatatypeDifference => "dt-diff",
            Self::ClassSchema => "scm-cls",
            Self::SubClassOfTransitive => "scm-sco",
            Self::EquivalentClassSchema => "scm-eqc1",
            Self::EquivalentClassSchemaReverse => "scm-eqc2",
            Self::ObjectPropertySchema => "scm-op",
            Self::DatatypePropertySchema => "scm-dp",
            Self::SubPropertyOfTransitive => "scm-spo",
            Self::EquivalentPropertySchema => "scm-eqp1",
            Self::EquivalentPropertySchemaReverse => "scm-eqp2",
            Self::DomainSchema => "scm-dom1",
            Self::DomainSchemaSubproperty => "scm-dom2",
            Self::RangeSchema => "scm-rng1",
            Self::RangeSchemaSubproperty => "scm-rng2",
            Self::HasValueSchema => "scm-hv",
            Self::SomeValuesSchema => "scm-svf1",
            Self::SomeValuesSchemaSubproperty => "scm-svf2",
            Self::AllValuesSchema => "scm-avf1",
            Self::AllValuesSchemaSubproperty => "scm-avf2",
            Self::IntersectionSchema => "scm-int",
            Self::UnionSchema => "scm-uni",
            Self::DatatypeIriTyping => "rdfs1",
            Self::SubjectResource => "rdfs4a",
            Self::ObjectResource => "rdfs4b",
            Self::PropertyReflexive => "rdfs6",
            Self::ClassResource => "rdfs8",
            Self::ClassReflexive => "rdfs10",
            Self::ContainerMembership => "rdfs12",
            Self::DatatypeLiteral => "rdfs13",
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
