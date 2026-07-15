#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSpec {
    pub simple_types: Vec<SimpleTypeSpec>,
    pub complex_types: Vec<ComplexTypeSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamProfileSchemaSpec {
    pub schema: SchemaSpec,
    pub root: ComplexTypeSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleTypeSpec {
    pub name: &'static str,
    pub kind: SimpleTypeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleTypeKind {
    Enumeration(&'static [&'static str]),
    DoubleList {
        length: usize,
    },
    IntegerRange {
        base: &'static str,
        min_inclusive: &'static str,
        max_inclusive: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComplexTypeSpec {
    pub name: &'static str,
    pub content: Option<ParticleSpec>,
    pub attributes: Vec<AttributeSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeSpec {
    pub name: &'static str,
    pub ty: &'static str,
    pub required: bool,
    pub default: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParticleSpec {
    Element(ElementSpec),
    Sequence(GroupSpec),
    Choice(GroupSpec),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSpec {
    pub min: usize,
    pub max: Option<usize>,
    pub children: Vec<ParticleSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementSpec {
    pub name: &'static str,
    pub ty: &'static str,
    pub min: usize,
    pub max: Option<usize>,
}

fn attr(name: &'static str, ty: &'static str) -> AttributeSpec {
    AttributeSpec {
        name,
        ty,
        required: false,
        default: None,
    }
}

fn required_attr(name: &'static str, ty: &'static str) -> AttributeSpec {
    AttributeSpec {
        name,
        ty,
        required: true,
        default: None,
    }
}

fn default_attr(name: &'static str, ty: &'static str, default: &'static str) -> AttributeSpec {
    AttributeSpec {
        name,
        ty,
        required: false,
        default: Some(default),
    }
}

fn element(name: &'static str, ty: &'static str, min: usize, max: Option<usize>) -> ParticleSpec {
    ParticleSpec::Element(ElementSpec { name, ty, min, max })
}

fn one(name: &'static str, ty: &'static str) -> ParticleSpec {
    element(name, ty, 1, Some(1))
}

fn optional(name: &'static str, ty: &'static str) -> ParticleSpec {
    element(name, ty, 0, Some(1))
}

fn many(name: &'static str, ty: &'static str) -> ParticleSpec {
    element(name, ty, 0, None)
}

fn one_or_more(name: &'static str, ty: &'static str) -> ParticleSpec {
    element(name, ty, 1, None)
}

fn sequence(children: Vec<ParticleSpec>) -> ParticleSpec {
    ParticleSpec::Sequence(GroupSpec {
        min: 1,
        max: Some(1),
        children,
    })
}

fn choice(children: Vec<ParticleSpec>) -> ParticleSpec {
    ParticleSpec::Choice(GroupSpec {
        min: 1,
        max: Some(1),
        children,
    })
}

fn optional_choice(children: Vec<ParticleSpec>) -> ParticleSpec {
    ParticleSpec::Choice(GroupSpec {
        min: 0,
        max: Some(1),
        children,
    })
}

fn complex(
    name: &'static str,
    content: Option<ParticleSpec>,
    attributes: Vec<AttributeSpec>,
) -> ComplexTypeSpec {
    ComplexTypeSpec {
        name,
        content,
        attributes,
    }
}

fn enum_type(name: &'static str, values: &'static [&'static str]) -> SimpleTypeSpec {
    SimpleTypeSpec {
        name,
        kind: SimpleTypeKind::Enumeration(values),
    }
}

fn integer_range(
    name: &'static str,
    base: &'static str,
    min_inclusive: &'static str,
    max_inclusive: &'static str,
) -> SimpleTypeSpec {
    SimpleTypeSpec {
        name,
        kind: SimpleTypeKind::IntegerRange {
            base,
            min_inclusive,
            max_inclusive,
        },
    }
}

pub fn connectivity_schema_spec() -> SchemaSpec {
    let simple_types = vec![
        enum_type(
            "Fidelity",
            &["functional", "presented", "exact", "quantified"],
        ),
        enum_type(
            "Purpose",
            &["communication", "power-delivery", "material-transfer"],
        ),
        enum_type(
            "Carrier",
            &[
                "electrical",
                "guided-optical",
                "conducted-rf",
                "radiated-rf",
                "liquid",
                "gas",
            ],
        ),
        enum_type(
            "TerminationMounting",
            &["endpoint", "inline", "branch", "closure"],
        ),
        enum_type("GptpClockKind", &["ordinary", "boundary", "transparent"]),
        enum_type("TrafficPreemption", &["express", "preemptable"]),
        enum_type(
            "MacsecEnforcement",
            &["must-secure", "should-secure", "integrity-only", "disabled"],
        ),
        enum_type("EeeMode", &["enabled", "disabled"]),
        enum_type("StreamProfileSelectionRole", &["default"]),
        SimpleTypeSpec {
            name: "ConnectivityVec3",
            kind: SimpleTypeKind::DoubleList { length: 3 },
        },
        SimpleTypeSpec {
            name: "ConnectivityQuat4",
            kind: SimpleTypeKind::DoubleList { length: 4 },
        },
    ];

    let mut complex_types = Vec::new();
    add_reference_types(&mut complex_types);
    add_quantity_and_selection_types(&mut complex_types);
    add_interface_and_representation_types(&mut complex_types);
    add_physical_types(&mut complex_types);
    add_network_configuration_types(&mut complex_types);
    add_topology_types(&mut complex_types);

    SchemaSpec {
        simple_types,
        complex_types,
    }
}

fn add_reference_types(types: &mut Vec<ComplexTypeSpec>) {
    types.push(complex(
        "connectivity_instance_segment",
        None,
        vec![
            attr("name", "xs:string"),
            required_attr("occurrence", "xs:unsignedInt"),
        ],
    ));
    types.push(complex(
        "connectivity_instance_ref",
        Some(sequence(vec![one_or_more(
            "segment",
            "connectivity_instance_segment",
        )])),
        vec![],
    ));

    for (name, key) in [
        ("connectivity_component_ref", "component"),
        ("connectivity_assembly_ref", "assembly"),
        ("connectivity_network_ref", "network"),
    ] {
        types.push(complex(
            name,
            Some(sequence(vec![optional(
                "instance",
                "connectivity_instance_ref",
            )])),
            vec![required_attr(key, "xs:string")],
        ));
    }

    types.push(complex(
        "connectivity_physical_owner_ref",
        Some(choice(vec![
            one("component-ref", "connectivity_component_ref"),
            one("assembly-ref", "connectivity_assembly_ref"),
        ])),
        vec![],
    ));

    for (name, second_attribute) in [
        ("connectivity_participant_ref", ("participant", "xs:string")),
        ("connectivity_hop_ref", ("hop", "xs:string")),
        ("connectivity_leg_ref", ("leg", "xs:string")),
        (
            "connectivity_traffic_class_ref",
            ("traffic-class", "xs:string"),
        ),
        ("connectivity_schedule_ref", ("schedule", "xs:string")),
        ("connectivity_macsec_policy_ref", ("policy", "xs:string")),
    ] {
        types.push(complex(
            name,
            Some(sequence(vec![optional(
                "instance",
                "connectivity_instance_ref",
            )])),
            vec![
                required_attr("network", "xs:string"),
                required_attr(second_attribute.0, second_attribute.1),
            ],
        ));
    }

    types.push(complex(
        "connectivity_function_ref",
        Some(sequence(vec![optional(
            "instance",
            "connectivity_instance_ref",
        )])),
        vec![
            required_attr("component", "xs:string"),
            required_attr("function", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_topology_segment_ref",
        Some(choice(vec![
            one("network-ref", "connectivity_network_ref"),
            one("leg-ref", "connectivity_leg_ref"),
        ])),
        vec![],
    ));

    types.push(complex(
        "connectivity_port_ref",
        Some(sequence(vec![optional(
            "instance",
            "connectivity_instance_ref",
        )])),
        vec![
            required_attr("component", "xs:string"),
            required_attr("port", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_channel_ref",
        Some(sequence(vec![optional(
            "instance",
            "connectivity_instance_ref",
        )])),
        vec![
            required_attr("component", "xs:string"),
            required_attr("port", "xs:string"),
            required_attr("channel", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_functional_endpoint_ref",
        Some(choice(vec![
            one("port-ref", "connectivity_port_ref"),
            one("channel-ref", "connectivity_channel_ref"),
        ])),
        vec![],
    ));

    types.push(complex(
        "connectivity_connector_ref",
        Some(sequence(vec![choice(vec![
            one("component-ref", "connectivity_component_ref"),
            one("assembly-ref", "connectivity_assembly_ref"),
        ])])),
        vec![required_attr("connector", "xs:string")],
    ));

    types.push(complex(
        "connectivity_position_ref",
        Some(sequence(vec![choice(vec![
            one("component-ref", "connectivity_component_ref"),
            one("assembly-ref", "connectivity_assembly_ref"),
        ])])),
        vec![
            required_attr("connector", "xs:string"),
            required_attr("position", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_junction_ref",
        Some(sequence(vec![choice(vec![
            one("component-ref", "connectivity_component_ref"),
            one("assembly-ref", "connectivity_assembly_ref"),
        ])])),
        vec![required_attr("junction", "xs:string")],
    ));
    types.push(complex(
        "connectivity_physical_endpoint_ref",
        Some(choice(vec![
            one("connector-ref", "connectivity_connector_ref"),
            one("position-ref", "connectivity_position_ref"),
            one("junction-ref", "connectivity_junction_ref"),
        ])),
        vec![],
    ));
}

fn add_quantity_and_selection_types(types: &mut Vec<ComplexTypeSpec>) {
    types.push(complex(
        "connectivity_quantity",
        None,
        vec![
            required_attr("value", "xs:double"),
            required_attr("unit", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_quantity_range",
        None,
        vec![
            attr("min", "xs:double"),
            attr("max", "xs:double"),
            attr("nominal", "xs:double"),
            required_attr("unit", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_selection_range",
        None,
        vec![
            required_attr("min", "xs:double"),
            required_attr("max", "xs:double"),
            attr("nominal", "xs:double"),
            required_attr("unit", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_selection_quantity",
        Some(choice(vec![
            one("nominal", "connectivity_quantity"),
            one("range", "connectivity_selection_range"),
        ])),
        vec![],
    ));

    types.push(complex(
        "connectivity_rf_numbered_channel",
        Some(sequence(vec![
            optional("center-frequency", "connectivity_selection_quantity"),
            optional("bandwidth", "connectivity_selection_quantity"),
        ])),
        vec![required_attr("number", "xs:unsignedInt")],
    ));
    types.push(complex(
        "connectivity_rf_frequency_defined_channel",
        Some(sequence(vec![
            one("center-frequency", "connectivity_selection_quantity"),
            optional("bandwidth", "connectivity_selection_quantity"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_rf_channel_selection",
        Some(choice(vec![
            one("numbered", "connectivity_rf_numbered_channel"),
            one(
                "frequency-defined",
                "connectivity_rf_frequency_defined_channel",
            ),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_rf_selection",
        Some(sequence(vec![one(
            "channel",
            "connectivity_rf_channel_selection",
        )])),
        vec![],
    ));

    types.push(complex(
        "connectivity_purpose_capability",
        None,
        vec![required_attr("value", "Purpose")],
    ));
    types.push(complex(
        "connectivity_carrier_capability",
        None,
        vec![required_attr("value", "Carrier")],
    ));
    types.push(complex(
        "connectivity_profile_capability",
        None,
        vec![required_attr("id", "xs:string")],
    ));
    types.push(complex(
        "connectivity_capabilities",
        Some(sequence(vec![
            many("purpose", "connectivity_purpose_capability"),
            many("carrier", "connectivity_carrier_capability"),
            many("profile", "connectivity_profile_capability"),
            optional("rate", "connectivity_quantity_range"),
            optional("voltage", "connectivity_quantity_range"),
            optional("current", "connectivity_quantity_range"),
            optional("power", "connectivity_quantity_range"),
            optional("impedance", "connectivity_quantity_range"),
            optional("frequency", "connectivity_quantity_range"),
            optional("bandwidth", "connectivity_quantity_range"),
            optional("pressure", "connectivity_quantity_range"),
            optional("flow", "connectivity_quantity_range"),
            optional("temperature", "connectivity_quantity_range"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_selected_profile",
        None,
        vec![required_attr("id", "xs:string")],
    ));
    types.push(complex(
        "connectivity_network_selection",
        Some(sequence(vec![
            many("profile", "connectivity_selected_profile"),
            optional("rate", "connectivity_selection_quantity"),
            optional("voltage", "connectivity_selection_quantity"),
            optional("current", "connectivity_selection_quantity"),
            optional("power", "connectivity_selection_quantity"),
            optional("impedance", "connectivity_selection_quantity"),
            optional("frequency", "connectivity_selection_quantity"),
            optional("rf", "connectivity_rf_selection"),
            optional("pressure", "connectivity_selection_quantity"),
            optional("flow", "connectivity_selection_quantity"),
            optional("temperature", "connectivity_selection_quantity"),
        ])),
        vec![
            required_attr("purpose", "Purpose"),
            required_attr("carrier", "Carrier"),
        ],
    ));
}

fn add_interface_and_representation_types(types: &mut Vec<ComplexTypeSpec>) {
    types.push(complex(
        "connectivity_channel",
        Some(sequence(vec![optional(
            "capabilities",
            "connectivity_capabilities",
        )])),
        vec![
            required_attr("name", "xs:string"),
            attr("role", "xs:string"),
            attr("local-group", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_port",
        Some(sequence(vec![
            optional("capabilities", "connectivity_capabilities"),
            many("channel", "connectivity_channel"),
        ])),
        vec![required_attr("name", "xs:string")],
    ));

    types.push(complex("connectivity_world_route_frame", None, vec![]));
    types.push(complex(
        "connectivity_component_origin_frame",
        Some(sequence(vec![optional(
            "instance",
            "connectivity_instance_ref",
        )])),
        vec![required_attr("component", "xs:string")],
    ));
    types.push(complex(
        "connectivity_component_named_frame",
        Some(sequence(vec![optional(
            "instance",
            "connectivity_instance_ref",
        )])),
        vec![
            required_attr("component", "xs:string"),
            required_attr("frame", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_route_frame",
        Some(choice(vec![
            one("world", "connectivity_world_route_frame"),
            one("component-origin", "connectivity_component_origin_frame"),
            one("component-frame", "connectivity_component_named_frame"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_rpy_rotation",
        None,
        vec![required_attr("value", "ConnectivityVec3")],
    ));
    types.push(complex(
        "connectivity_quaternion_rotation",
        None,
        vec![required_attr("value", "ConnectivityQuat4")],
    ));
    types.push(complex(
        "connectivity_placement_rotation",
        Some(choice(vec![
            one("rpy", "connectivity_rpy_rotation"),
            one("quaternion", "connectivity_quaternion_rotation"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_placement",
        Some(sequence(vec![
            one("frame", "connectivity_route_frame"),
            one("rotation", "connectivity_placement_rotation"),
        ])),
        vec![required_attr("xyz", "ConnectivityVec3")],
    ));
    types.push(complex(
        "connectivity_route_point",
        Some(sequence(vec![
            one("frame", "connectivity_route_frame"),
            optional("rotation", "connectivity_placement_rotation"),
        ])),
        vec![required_attr("xyz", "ConnectivityVec3")],
    ));

    types.push(complex(
        "connectivity_primitive_box",
        Some(sequence(vec![one("placement", "connectivity_placement")])),
        vec![required_attr("size", "ConnectivityVec3")],
    ));
    types.push(complex(
        "connectivity_primitive_cylinder",
        Some(sequence(vec![one("placement", "connectivity_placement")])),
        vec![
            required_attr("radius", "xs:double"),
            required_attr("length", "xs:double"),
        ],
    ));
    types.push(complex(
        "connectivity_primitive_sphere",
        Some(sequence(vec![one("placement", "connectivity_placement")])),
        vec![required_attr("radius", "xs:double")],
    ));
    types.push(complex(
        "connectivity_model_representation",
        Some(sequence(vec![one("placement", "connectivity_placement")])),
        vec![
            required_attr("uri", "xs:anyURI"),
            attr("sha", "xs:string"),
            attr("node-path", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_component_visual_root",
        Some(sequence(vec![optional(
            "instance",
            "connectivity_instance_ref",
        )])),
        vec![
            required_attr("component", "xs:string"),
            required_attr("visual", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_assembly_model_root",
        Some(sequence(vec![optional(
            "instance",
            "connectivity_instance_ref",
        )])),
        vec![required_attr("assembly", "xs:string")],
    ));
    types.push(complex(
        "connectivity_model_root",
        Some(choice(vec![
            one("component-visual", "connectivity_component_visual_root"),
            one("assembly-model", "connectivity_assembly_model_root"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_model_part_representation",
        Some(sequence(vec![one("model-root", "connectivity_model_root")])),
        vec![
            required_attr("node-path", "xs:string"),
            attr("submesh-fallback", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_round_route_section",
        None,
        vec![required_attr("diameter", "xs:double")],
    ));
    types.push(complex(
        "connectivity_rectangular_route_section",
        None,
        vec![
            required_attr("width", "xs:double"),
            required_attr("height", "xs:double"),
        ],
    ));
    types.push(complex(
        "connectivity_derived_route_representation",
        Some(sequence(vec![
            optional_choice(vec![
                one("round-section", "connectivity_round_route_section"),
                one(
                    "rectangular-section",
                    "connectivity_rectangular_route_section",
                ),
            ]),
            element("waypoint", "connectivity_route_point", 2, None),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_representation",
        Some(choice(vec![
            one("box", "connectivity_primitive_box"),
            one("cylinder", "connectivity_primitive_cylinder"),
            one("sphere", "connectivity_primitive_sphere"),
            one("model", "connectivity_model_representation"),
            one("model-part", "connectivity_model_part_representation"),
            one("derived-route", "connectivity_derived_route_representation"),
        ])),
        vec![],
    ));

    types.push(complex(
        "connectivity_position",
        Some(sequence(vec![optional(
            "representation",
            "connectivity_representation",
        )])),
        vec![
            required_attr("name", "xs:string"),
            attr("role", "xs:string"),
            attr("local-group", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_connector",
        Some(sequence(vec![
            many("pin", "connectivity_position"),
            many("socket", "connectivity_position"),
            many("contact", "connectivity_position"),
            many("fiber", "connectivity_position"),
            many("passage", "connectivity_position"),
            many("feed", "connectivity_position"),
            many("waveguide-opening", "connectivity_position"),
            optional("representation", "connectivity_representation"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            attr("family", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_antenna",
        Some(sequence(vec![
            optional("conducted-port", "connectivity_port_ref"),
            one("radiated-port", "connectivity_port_ref"),
            optional("representation", "connectivity_representation"),
        ])),
        vec![required_attr("name", "xs:string")],
    ));
    types.push(complex(
        "connectivity_function",
        Some(sequence(vec![
            many("input", "connectivity_functional_endpoint_ref"),
            many("output", "connectivity_functional_endpoint_ref"),
            many("bidirectional", "connectivity_functional_endpoint_ref"),
        ])),
        vec![required_attr("name", "xs:string")],
    ));
}

fn add_physical_types(types: &mut Vec<ComplexTypeSpec>) {
    types.push(complex(
        "connectivity_path",
        Some(sequence(vec![
            one("first", "connectivity_physical_endpoint_ref"),
            one("second", "connectivity_physical_endpoint_ref"),
            optional("representation", "connectivity_representation"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("fidelity", "Fidelity"),
            attr("role", "xs:string"),
            attr("local-group", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_junction",
        Some(sequence(vec![
            one_or_more("attachment", "connectivity_physical_endpoint_ref"),
            optional("representation", "connectivity_representation"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("fidelity", "Fidelity"),
        ],
    ));
    types.push(complex(
        "connectivity_named_quantity",
        None,
        vec![
            required_attr("property", "xs:string"),
            required_attr("value", "xs:double"),
            required_attr("unit", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_termination",
        Some(sequence(vec![
            one_or_more("attachment", "connectivity_physical_endpoint_ref"),
            many("quantity", "connectivity_named_quantity"),
            optional("representation", "connectivity_representation"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("kind", "xs:string"),
            required_attr("mounting", "TerminationMounting"),
            required_attr("fidelity", "Fidelity"),
            attr("profile", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_assembly",
        Some(sequence(vec![
            many("connector", "connectivity_connector"),
            many("wire", "connectivity_path"),
            many("conductor", "connectivity_path"),
            many("cable-member", "connectivity_path"),
            many("fiber", "connectivity_path"),
            many("coax", "connectivity_path"),
            many("waveguide", "connectivity_path"),
            many("feed", "connectivity_path"),
            many("hose", "connectivity_path"),
            many("pipe", "connectivity_path"),
            many("passage", "connectivity_path"),
            many("splice", "connectivity_junction"),
            many("tee", "connectivity_junction"),
            many("manifold", "connectivity_junction"),
            many("busbar", "connectivity_junction"),
            many("optical-splitter", "connectivity_junction"),
            many("termination", "connectivity_termination"),
            optional("representation", "connectivity_representation"),
        ])),
        vec![required_attr("name", "xs:string")],
    ));
    types.push(complex(
        "connectivity_binding",
        Some(sequence(vec![
            one("functional", "connectivity_functional_endpoint_ref"),
            one("physical", "connectivity_physical_endpoint_ref"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("fidelity", "Fidelity"),
        ],
    ));
    types.push(complex(
        "connectivity_position_mapping",
        Some(sequence(vec![
            one("first", "connectivity_position_ref"),
            one("second", "connectivity_position_ref"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_mate",
        Some(sequence(vec![
            one("first", "connectivity_connector_ref"),
            one("second", "connectivity_connector_ref"),
            many("position-mapping", "connectivity_position_mapping"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("fidelity", "Fidelity"),
        ],
    ));

    types.push(complex(
        "connectivity_hop_owner_ref",
        Some(choice(vec![
            one("component-ref", "connectivity_component_ref"),
            one("function-ref", "connectivity_function_ref"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_participant",
        Some(sequence(vec![one(
            "endpoint",
            "connectivity_functional_endpoint_ref",
        )])),
        vec![
            required_attr("name", "xs:string"),
            attr("role", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_hop",
        Some(sequence(vec![
            optional("description", "xs:string"),
            one("owner", "connectivity_hop_owner_ref"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            attr("role", "xs:string"),
            attr("processing-delay-ns", "xs:unsignedLong"),
        ],
    ));
    types.push(complex(
        "connectivity_leg_end",
        Some(sequence(vec![
            one("hop-ref", "connectivity_hop_ref"),
            one("participant-ref", "connectivity_participant_ref"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_leg",
        Some(sequence(vec![
            one("from", "connectivity_leg_end"),
            one("to", "connectivity_leg_end"),
        ])),
        vec![required_attr("name", "xs:string")],
    ));
    types.push(complex(
        "connectivity_participant_reference",
        Some(sequence(vec![one(
            "participant-ref",
            "connectivity_participant_ref",
        )])),
        vec![],
    ));
    types.push(complex(
        "connectivity_hop_reference",
        Some(sequence(vec![one("hop-ref", "connectivity_hop_ref")])),
        vec![],
    ));
}

fn add_network_configuration_types(types: &mut Vec<ComplexTypeSpec>) {
    types.push(complex(
        "connectivity_gptp_clock",
        Some(sequence(vec![one(
            "participant-ref",
            "connectivity_participant_ref",
        )])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("kind", "GptpClockKind"),
            required_attr("gm-capable", "xs:boolean"),
            attr("priority1", "xs:unsignedByte"),
            attr("priority2", "xs:unsignedByte"),
            attr("clock-class", "xs:unsignedByte"),
            attr("clock-accuracy", "xs:unsignedByte"),
        ],
    ));
    types.push(complex(
        "connectivity_gptp_port_defaults",
        None,
        vec![
            attr("log-sync-interval", "xs:byte"),
            attr("log-announce-interval", "xs:byte"),
            attr("log-pdelay-req-interval", "xs:byte"),
            attr("announce-receipt-timeout", "xs:unsignedByte"),
            attr("neighbor-prop-delay-threshold-ns", "xs:unsignedLong"),
        ],
    ));
    types.push(complex(
        "connectivity_gptp_domain",
        Some(sequence(vec![
            many("clock", "connectivity_gptp_clock"),
            optional("port-defaults", "connectivity_gptp_port_defaults"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("number", "xs:unsignedByte"),
        ],
    ));
    types.push(complex(
        "connectivity_pcp_value",
        None,
        vec![required_attr("value", "xs:unsignedByte")],
    ));
    types.push(complex(
        "connectivity_traffic_class",
        Some(sequence(vec![
            optional("description", "xs:string"),
            many("pcp", "connectivity_pcp_value"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("number", "xs:unsignedByte"),
            attr("preemption", "TrafficPreemption"),
        ],
    ));
    types.push(complex(
        "connectivity_open_traffic_classes",
        Some(sequence(vec![many(
            "traffic-class-ref",
            "connectivity_traffic_class_ref",
        )])),
        vec![],
    ));
    types.push(complex(
        "connectivity_gate_control_entry",
        Some(sequence(vec![one(
            "open",
            "connectivity_open_traffic_classes",
        )])),
        vec![required_attr("duration-ns", "xs:unsignedLong")],
    ));
    types.push(complex(
        "connectivity_gate_schedule",
        Some(sequence(vec![many(
            "gate",
            "connectivity_gate_control_entry",
        )])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("cycle-time-ns", "xs:unsignedLong"),
        ],
    ));
    types.push(complex(
        "connectivity_schedule_target",
        Some(sequence(vec![one(
            "participant-ref",
            "connectivity_participant_ref",
        )])),
        vec![],
    ));
    types.push(complex(
        "connectivity_schedule_assignment",
        Some(sequence(vec![
            one("schedule-ref", "connectivity_schedule_ref"),
            many("target", "connectivity_schedule_target"),
        ])),
        vec![required_attr("name", "xs:string")],
    ));
    types.push(complex(
        "connectivity_plca_node",
        Some(sequence(vec![one(
            "participant-ref",
            "connectivity_participant_ref",
        )])),
        vec![
            required_attr("id", "xs:unsignedByte"),
            required_attr("burst-count", "xs:unsignedByte"),
            required_attr("burst-timer-bit-times", "xs:unsignedShort"),
        ],
    ));
    types.push(complex(
        "connectivity_plca_configuration",
        Some(sequence(vec![many("node", "connectivity_plca_node")])),
        vec![
            required_attr("max-node-id", "xs:unsignedByte"),
            required_attr("to-timer-bit-times", "xs:unsignedShort"),
        ],
    ));
    types.push(complex(
        "connectivity_macsec_policy_definition",
        None,
        vec![
            required_attr("name", "xs:string"),
            required_attr("enforcement", "MacsecEnforcement"),
            attr("cipher", "xs:string"),
            attr("key-agreement", "xs:string"),
            attr("confidentiality-offset", "xs:unsignedByte"),
            attr("rekey-interval-ns", "xs:unsignedLong"),
            attr("credential-store-ref", "xs:string"),
        ],
    ));
    types.push(complex(
        "connectivity_macsec_default_policy",
        Some(sequence(vec![one(
            "macsec-policy-ref",
            "connectivity_macsec_policy_ref",
        )])),
        vec![],
    ));
    types.push(complex(
        "connectivity_macsec_override",
        Some(sequence(vec![
            one("target", "connectivity_topology_segment_ref"),
            one("macsec-policy-ref", "connectivity_macsec_policy_ref"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_macsec_configuration",
        Some(sequence(vec![
            many("policy", "connectivity_macsec_policy_definition"),
            optional("default-policy", "connectivity_macsec_default_policy"),
            many("override", "connectivity_macsec_override"),
        ])),
        vec![],
    ));
    types.push(complex(
        "connectivity_eee_override",
        Some(sequence(vec![one(
            "participant-ref",
            "connectivity_participant_ref",
        )])),
        vec![required_attr("mode", "EeeMode")],
    ));
    types.push(complex(
        "connectivity_eee_configuration",
        Some(sequence(vec![many(
            "override",
            "connectivity_eee_override",
        )])),
        vec![required_attr("default-mode", "EeeMode")],
    ));
    types.push(complex(
        "connectivity_network_configuration",
        Some(sequence(vec![
            many("gptp-domain", "connectivity_gptp_domain"),
            many("traffic-class", "connectivity_traffic_class"),
            many("gate-schedule", "connectivity_gate_schedule"),
            many("schedule-assignment", "connectivity_schedule_assignment"),
            optional("plca", "connectivity_plca_configuration"),
            optional("macsec", "connectivity_macsec_configuration"),
            optional("eee", "connectivity_eee_configuration"),
        ])),
        vec![],
    ));
}

fn add_topology_types(types: &mut Vec<ComplexTypeSpec>) {
    let network_attributes = || vec![required_attr("name", "xs:string")];
    let network_prefix = || {
        vec![
            optional("description", "xs:string"),
            one("selected", "connectivity_network_selection"),
            optional("configuration", "connectivity_network_configuration"),
        ]
    };

    let mut link = network_prefix();
    link.push(element(
        "participant",
        "connectivity_participant",
        2,
        Some(2),
    ));
    types.push(complex(
        "connectivity_link",
        Some(sequence(link)),
        network_attributes(),
    ));

    let mut bus = network_prefix();
    bus.push(element("participant", "connectivity_participant", 2, None));
    types.push(complex(
        "connectivity_bus",
        Some(sequence(bus)),
        network_attributes(),
    ));

    let mut chain = network_prefix();
    chain.extend([
        element("participant", "connectivity_participant", 2, None),
        element("hop", "connectivity_hop", 2, None),
        one_or_more("leg", "connectivity_leg"),
    ]);
    types.push(complex(
        "connectivity_chain",
        Some(sequence(chain)),
        network_attributes(),
    ));

    let mut star = network_prefix();
    star.extend([
        one("coordinator", "connectivity_participant_reference"),
        element("participant", "connectivity_participant", 2, None),
    ]);
    types.push(complex(
        "connectivity_star",
        Some(sequence(star)),
        network_attributes(),
    ));

    let mut ring = network_prefix();
    ring.extend([
        element("participant", "connectivity_participant", 3, None),
        element("hop", "connectivity_hop", 3, None),
        element("leg", "connectivity_leg", 3, None),
    ]);
    types.push(complex(
        "connectivity_ring",
        Some(sequence(ring)),
        network_attributes(),
    ));

    let mut mesh = network_prefix();
    mesh.push(element("participant", "connectivity_participant", 2, None));
    types.push(complex(
        "connectivity_mesh",
        Some(sequence(mesh)),
        network_attributes(),
    ));

    let mut tree = network_prefix();
    tree.extend([
        one("root", "connectivity_hop_reference"),
        element("participant", "connectivity_participant", 2, None),
        element("hop", "connectivity_hop", 2, None),
        one_or_more("leg", "connectivity_leg"),
    ]);
    types.push(complex(
        "connectivity_tree",
        Some(sequence(tree)),
        network_attributes(),
    ));

    types.push(complex(
        "connectivity_stream_profile_resource",
        None,
        vec![
            required_attr("uri", "xs:anyURI"),
            attr("sha", "xs:string"),
            default_attr("required", "xs:boolean", "true"),
            attr("selection-role", "StreamProfileSelectionRole"),
        ],
    ));
}

pub fn component_connectivity_particles() -> Vec<ParticleSpec> {
    vec![
        many("port", "connectivity_port"),
        many("connector", "connectivity_connector"),
        many("antenna", "connectivity_antenna"),
        many("switch", "connectivity_function"),
        many("bridge", "connectivity_function"),
        many("converter", "connectivity_function"),
        many("transceiver", "connectivity_function"),
        many("radio", "connectivity_function"),
        many("wire", "connectivity_path"),
        many("conductor", "connectivity_path"),
        many("cable-member", "connectivity_path"),
        many("fiber", "connectivity_path"),
        many("coax", "connectivity_path"),
        many("waveguide", "connectivity_path"),
        many("feed", "connectivity_path"),
        many("hose", "connectivity_path"),
        many("pipe", "connectivity_path"),
        many("passage", "connectivity_path"),
        many("splice", "connectivity_junction"),
        many("tee", "connectivity_junction"),
        many("manifold", "connectivity_junction"),
        many("busbar", "connectivity_junction"),
        many("optical-splitter", "connectivity_junction"),
        many("termination", "connectivity_termination"),
    ]
}

pub fn root_connectivity_particles() -> Vec<ParticleSpec> {
    vec![
        many("harness", "connectivity_assembly"),
        many("cable", "connectivity_assembly"),
        many("plumbing", "connectivity_assembly"),
        many("umbilical", "connectivity_assembly"),
        many("binding", "connectivity_binding"),
        many("mate", "connectivity_mate"),
        many("link", "connectivity_link"),
        many("bus", "connectivity_bus"),
        many("chain", "connectivity_chain"),
        many("star", "connectivity_star"),
        many("ring", "connectivity_ring"),
        many("mesh", "connectivity_mesh"),
        many("tree", "connectivity_tree"),
        many("stream-profile", "connectivity_stream_profile_resource"),
    ]
}

pub fn stream_profile_schema_spec() -> StreamProfileSchemaSpec {
    let simple_types = vec![
        enum_type("FrerSequenceEncoding", &["r-tag", "hsr"]),
        integer_range("StreamVlanId", "xs:unsignedShort", "0", "4094"),
        integer_range("StreamPcp", "xs:unsignedByte", "0", "7"),
        integer_range(
            "StreamMaxFrameSizeBytes",
            "xs:unsignedInt",
            "1",
            "4294967295",
        ),
        integer_range(
            "StreamIntervalNs",
            "xs:unsignedLong",
            "1",
            "18446744073709551615",
        ),
        integer_range(
            "StreamMaxLatencyNs",
            "xs:unsignedLong",
            "1",
            "18446744073709551615",
        ),
        integer_range("FrerSeamlessTrees", "xs:unsignedInt", "2", "4294967295"),
    ];
    let mut complex_types = Vec::new();

    complex_types.push(complex(
        "connectivity_instance_segment",
        None,
        vec![
            attr("name", "xs:string"),
            required_attr("occurrence", "xs:unsignedInt"),
        ],
    ));
    complex_types.push(complex(
        "connectivity_instance_ref",
        Some(sequence(vec![one_or_more(
            "segment",
            "connectivity_instance_segment",
        )])),
        vec![],
    ));
    for (name, primary, secondary) in [
        ("connectivity_network_ref", ("network", "xs:string"), None),
        (
            "connectivity_participant_ref",
            ("network", "xs:string"),
            Some(("participant", "xs:string")),
        ),
        (
            "connectivity_traffic_class_ref",
            ("network", "xs:string"),
            Some(("traffic-class", "xs:string")),
        ),
        (
            "connectivity_schedule_ref",
            ("network", "xs:string"),
            Some(("schedule", "xs:string")),
        ),
    ] {
        let mut attributes = vec![required_attr(primary.0, primary.1)];
        if let Some((attribute, ty)) = secondary {
            attributes.push(required_attr(attribute, ty));
        }
        complex_types.push(complex(
            name,
            Some(sequence(vec![optional(
                "instance",
                "connectivity_instance_ref",
            )])),
            attributes,
        ));
    }
    complex_types.push(complex(
        "connectivity_function_ref",
        Some(sequence(vec![optional(
            "instance",
            "connectivity_instance_ref",
        )])),
        vec![
            required_attr("component", "xs:string"),
            required_attr("function", "xs:string"),
        ],
    ));
    complex_types.push(complex(
        "stream_profile_dependency",
        None,
        vec![
            required_attr("uri", "xs:anyURI"),
            attr("sha", "xs:string"),
            default_attr("required", "xs:boolean", "true"),
        ],
    ));
    complex_types.push(complex(
        "stream_profile_group",
        Some(sequence(vec![optional("description", "xs:string")])),
        vec![required_attr("name", "xs:string")],
    ));
    complex_types.push(complex(
        "stream_profile_group_ref",
        Some(sequence(vec![optional(
            "instance",
            "connectivity_instance_ref",
        )])),
        vec![required_attr("group", "xs:string")],
    ));
    complex_types.push(complex(
        "stream_profile_forwarding_end",
        Some(sequence(vec![one(
            "participant-ref",
            "connectivity_participant_ref",
        )])),
        vec![],
    ));
    complex_types.push(complex(
        "stream_profile_forwarding",
        Some(sequence(vec![
            one("from", "stream_profile_forwarding_end"),
            one("function-ref", "connectivity_function_ref"),
            one("to", "stream_profile_forwarding_end"),
        ])),
        vec![],
    ));
    complex_types.push(complex(
        "stream_profile_path",
        Some(sequence(vec![
            one_or_more("network-ref", "connectivity_network_ref"),
            many("forwarding", "stream_profile_forwarding"),
        ])),
        vec![],
    ));
    complex_types.push(complex(
        "stream_profile_talker",
        Some(sequence(vec![one(
            "participant-ref",
            "connectivity_participant_ref",
        )])),
        vec![],
    ));
    complex_types.push(complex(
        "stream_profile_listener",
        Some(sequence(vec![one(
            "participant-ref",
            "connectivity_participant_ref",
        )])),
        vec![],
    ));
    complex_types.push(complex(
        "stream_profile_frer",
        None,
        vec![
            required_attr("seamless-trees", "FrerSeamlessTrees"),
            required_attr("sequence-encoding", "FrerSequenceEncoding"),
        ],
    ));
    complex_types.push(complex(
        "stream_profile_stream",
        Some(sequence(vec![
            optional("description", "xs:string"),
            optional("group-ref", "stream_profile_group_ref"),
            one("path", "stream_profile_path"),
            one("talker", "stream_profile_talker"),
            one_or_more("listener", "stream_profile_listener"),
            optional("traffic-class-ref", "connectivity_traffic_class_ref"),
            optional("schedule-ref", "connectivity_schedule_ref"),
            optional("frer", "stream_profile_frer"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            attr("vlan-id", "StreamVlanId"),
            attr("pcp", "StreamPcp"),
            required_attr("max-frame-size-bytes", "StreamMaxFrameSizeBytes"),
            required_attr("interval-ns", "StreamIntervalNs"),
            attr("max-latency-ns", "StreamMaxLatencyNs"),
            attr("protocol", "xs:string"),
        ],
    ));

    let root = complex(
        "stream-profile",
        Some(sequence(vec![
            optional("description", "xs:string"),
            many("dependency", "stream_profile_dependency"),
            many("stream-group", "stream_profile_group"),
            many("stream", "stream_profile_stream"),
        ])),
        vec![
            required_attr("name", "xs:string"),
            required_attr("version", "xs:string"),
        ],
    );

    StreamProfileSchemaSpec {
        schema: SchemaSpec {
            simple_types,
            complex_types,
        },
        root,
    }
}
