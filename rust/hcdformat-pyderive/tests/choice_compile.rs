use hcdformat_pyderive::{pychoice, pydom};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[derive(Debug, Clone, PartialEq)]
struct Child {
    name: String,
}

trait DomEnum: Sized {
    fn dom_value(&self) -> &'static str;
    fn from_dom_value(value: &str) -> Option<Self>;
}

#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Manual,
    Automatic,
}

impl DomEnum for Mode {
    fn dom_value(&self) -> &'static str {
        match self {
            Self::Manual => "manual-control",
            Self::Automatic => "automatic-control",
        }
    }

    fn from_dom_value(value: &str) -> Option<Self> {
        match value {
            "manual-control" => Some(Self::Manual),
            "automatic-control" => Some(Self::Automatic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Choice {
    Payload(Child),
    Text(String),
    Scalar(u32),
    Mode(Mode),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
struct WorldRouteFrame {}

#[derive(Debug, Clone, PartialEq)]
struct ComponentNamedFrame {
    component: String,
    frame: String,
}

#[derive(Debug, Clone, PartialEq)]
enum RouteFrameChoice {
    World(WorldRouteFrame),
    ComponentFrame(ComponentNamedFrame),
}

#[derive(Debug, Clone, PartialEq)]
struct RouteFrame {
    frame: RouteFrameChoice,
}

#[derive(Debug, Clone, PartialEq)]
struct Hcdf {
    choice: Choice,
    optional_choice: Option<Choice>,
    route_frame: RouteFrame,
}

impl Hcdf {
    fn from_xml_str(_xml: &str) -> Result<Self, String> {
        Ok(Self {
            choice: Choice::Empty,
            optional_choice: None,
            route_frame: RouteFrame {
                frame: RouteFrameChoice::ComponentFrame(ComponentNamedFrame {
                    component: String::new(),
                    frame: String::new(),
                }),
            },
        })
    }

    fn to_xml_string(&self) -> Result<String, String> {
        Ok(String::new())
    }
}

fn draft_child() -> Child {
    Child {
        name: String::new(),
    }
}

fn draft_world_route_frame() -> WorldRouteFrame {
    WorldRouteFrame {}
}

fn draft_component_named_frame() -> ComponentNamedFrame {
    ComponentNamedFrame {
        component: String::new(),
        frame: String::new(),
    }
}

fn err(error: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(error.to_string())
}

fn stale(name: &str) -> PyErr {
    PyValueError::new_err(format!("stale handle: {name}"))
}

fn resolve_index(index: isize, len: usize) -> PyResult<usize> {
    let index = if index < 0 {
        index + len as isize
    } else {
        index
    };
    if index < 0 || index as usize >= len {
        return Err(PyValueError::new_err("index out of range"));
    }
    Ok(index as usize)
}

trait Resolve<Root>: Sized {
    type Loc;
    fn resolve<'a>(root: &'a Root, loc: &Self::Loc) -> Option<&'a Self>;
    fn resolve_mut<'a>(root: &'a mut Root, loc: &Self::Loc) -> Option<&'a mut Self>;
}

#[pydom(
    py = PyHcdf,
    target = Hcdf,
    owner(target = Hcdf, py = PyHcdf),
    root
)]
struct HcdfDom {
    #[pydom(choice = PyChoice)]
    choice: (),
    #[pydom(choice = PyChoice)]
    optional_choice: Option<()>,
    #[pydom(required_nested = PyRouteFrame)]
    route_frame: (),
}

#[pydom(
    py = PyChild,
    target = Child,
    owner(target = Hcdf, py = PyHcdf),
    choice_site(parent = PyChoice, ptype = Choice, variant = Payload),
)]
struct ChildDom {
    name: String,
}

#[pychoice(
    py = PyChoice,
    target = Choice,
    owner(target = Hcdf, py = PyHcdf),
    site(parent = PyHcdf, ptype = Hcdf, field = choice, card = req),
    site(parent = PyHcdf, ptype = Hcdf, field = optional_choice, card = opt),
)]
enum ChoiceDom {
    #[pychoice(name = "payload", payload = PyChild, draft = draft_child)]
    Payload(Child),
    #[pychoice(name = "text")]
    Text(String),
    #[pychoice(name = "scalar")]
    Scalar(u32),
    #[pychoice(name = "mode", mapped_enum)]
    Mode(Mode),
    #[pychoice(name = "empty")]
    Empty,
}

#[pydom(
    py = PyRouteFrame,
    target = RouteFrame,
    owner(target = Hcdf, py = PyHcdf),
    site(parent = PyHcdf, ptype = Hcdf, field = route_frame, card = req),
)]
struct RouteFrameDom {
    #[pydom(choice = PyRouteFrameChoice)]
    frame: (),
}

#[pydom(
    py = PyWorldRouteFrame,
    target = WorldRouteFrame,
    owner(target = Hcdf, py = PyHcdf),
    choice_site(
        parent = PyRouteFrameChoice,
        ptype = RouteFrameChoice,
        variant = World
    ),
)]
struct WorldRouteFrameDom {}

#[pydom(
    py = PyComponentNamedFrame,
    target = ComponentNamedFrame,
    owner(target = Hcdf, py = PyHcdf),
    choice_site(
        parent = PyRouteFrameChoice,
        ptype = RouteFrameChoice,
        variant = ComponentFrame
    ),
)]
struct ComponentNamedFrameDom {
    component: String,
    frame: String,
}

#[pychoice(
    py = PyRouteFrameChoice,
    target = RouteFrameChoice,
    owner(target = Hcdf, py = PyHcdf),
    site(
        parent = PyRouteFrame,
        ptype = RouteFrame,
        field = frame,
        card = req
    ),
)]
enum RouteFrameChoiceDom {
    #[pychoice(
        name = "world",
        payload = PyWorldRouteFrame,
        draft = draft_world_route_frame
    )]
    World(WorldRouteFrame),
    #[pychoice(
        name = "component_frame",
        payload = PyComponentNamedFrame,
        draft = draft_component_named_frame
    )]
    ComponentFrame(ComponentNamedFrame),
}

#[test]
fn exact_choice_converter_accepts_one_arm() {
    assert_eq!(resolve_index(-1, 1).unwrap(), 0);
    let payload = Child {
        name: "child".to_owned(),
    };
    assert_eq!(
        PyChoice::__from_arms(Some(payload.clone()), None, None, None, false).unwrap(),
        Choice::Payload(payload)
    );
    assert_eq!(
        PyChoice::__from_arms(None, None, Some(7), None, false).unwrap(),
        Choice::Scalar(7)
    );
    assert_eq!(
        PyChoice::__from_arms(None, None, None, Some(Mode::Automatic), false).unwrap(),
        Choice::Mode(Mode::Automatic)
    );
    assert_eq!(
        PyChoice::__from_arms(None, None, None, None, true).unwrap(),
        Choice::Empty
    );
}

#[test]
fn exact_choice_converter_rejects_zero_or_multiple_arms() {
    let zero = PyChoice::__from_arms(None, None, None, None, false).unwrap_err();
    assert!(zero.contains("exactly one selected arm, got 0"), "{zero}");

    let both = PyChoice::__from_arms(
        Some(Child {
            name: "child".to_owned(),
        }),
        None,
        Some(7),
        None,
        false,
    )
    .unwrap_err();
    assert!(both.contains("exactly one selected arm, got 2"), "{both}");
}

#[test]
fn binding_authors_every_arm_and_enforces_handle_lifetimes() {
    Python::with_gil(|py| {
        let root = Py::new(py, PyHcdf::loads("<hcdf/>").unwrap()).unwrap();
        let root_bound = root.bind(py);
        let choice = root_bound.getattr("choice").unwrap();

        let payload = choice.call_method0("select_payload").unwrap();
        assert_eq!(
            payload
                .getattr("name")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            ""
        );
        payload.setattr("name", "draft-edited").unwrap();
        assert_eq!(
            choice
                .getattr("variant")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "payload"
        );

        choice.call_method1("select_text", ("text-value",)).unwrap();
        assert_eq!(
            choice.getattr("text").unwrap().extract::<String>().unwrap(),
            "text-value"
        );
        assert!(
            payload.getattr("name").is_err(),
            "old struct-arm handle must be stale"
        );

        choice.call_method1("select_scalar", (17_u32,)).unwrap();
        assert_eq!(
            choice.getattr("scalar").unwrap().extract::<u32>().unwrap(),
            17
        );
        choice
            .call_method1("select_mode", ("automatic-control",))
            .unwrap();
        assert_eq!(
            choice.getattr("mode").unwrap().extract::<String>().unwrap(),
            "automatic-control"
        );
        choice.call_method0("select_empty").unwrap();
        assert!(choice.getattr("empty").unwrap().extract::<bool>().unwrap());

        assert!(root_bound.getattr("optional_choice").unwrap().is_none());
        let optional = root_bound.call_method0("edit_optional_choice").unwrap();
        let optional_payload = optional.call_method0("select_payload").unwrap();
        optional_payload.setattr("name", "optional").unwrap();
        assert!(!root_bound.getattr("optional_choice").unwrap().is_none());
        root_bound.setattr("optional_choice", py.None()).unwrap();
        assert!(root_bound.getattr("optional_choice").unwrap().is_none());
        assert!(
            optional_payload.getattr("name").is_err(),
            "clearing an optional choice must stale its payload handles"
        );
        assert!(
            root_bound.setattr("choice", py.None()).is_err(),
            "required choices cannot be cleared"
        );
    });
}

#[test]
fn binding_clones_struct_arms_across_documents_explicitly() {
    Python::with_gil(|py| {
        let source = Py::new(py, PyHcdf::loads("<hcdf/>").unwrap()).unwrap();
        let target = Py::new(py, PyHcdf::loads("<hcdf/>").unwrap()).unwrap();
        let source_choice = source.bind(py).getattr("choice").unwrap();
        let target_choice = target.bind(py).getattr("choice").unwrap();

        let source_payload = source_choice.call_method0("select_payload").unwrap();
        source_payload.setattr("name", "source").unwrap();
        let cloned_payload = target_choice
            .call_method1("set_payload_from", (source_payload.clone(),))
            .unwrap();
        cloned_payload.setattr("name", "target").unwrap();

        assert_eq!(
            source_payload
                .getattr("name")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "source"
        );
        assert_eq!(
            cloned_payload
                .getattr("name")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "target"
        );
        let source_doc = source.borrow(py);
        let target_doc = target.borrow(py);
        assert!(matches!(
            &source_doc.doc.borrow().choice,
            Choice::Payload(Child { name }) if name == "source"
        ));
        assert!(matches!(
            &target_doc.doc.borrow().choice,
            Choice::Payload(Child { name }) if name == "target"
        ));
    });
}

#[test]
fn fresh_document_selects_world_route_frame_and_stales_previous_arm() {
    Python::with_gil(|py| {
        let root = Py::new(py, PyHcdf::loads("<hcdf/>").unwrap()).unwrap();
        let route_frame = root.bind(py).getattr("route_frame").unwrap();
        let frame = route_frame.getattr("frame").unwrap();
        let component = frame.call_method0("select_component_frame").unwrap();
        component.setattr("component", "arm").unwrap();
        component.setattr("frame", "tool").unwrap();

        let world = frame.call_method0("select_world").unwrap();
        assert!(world
            .repr()
            .unwrap()
            .extract::<String>()
            .unwrap()
            .contains("PyWorldRouteFrame"));
        assert!(
            component.getattr("component").is_err(),
            "switching to world must stale the old component-frame handle"
        );
        assert_eq!(
            frame
                .getattr("variant")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "world"
        );

        let root_ref = root.borrow(py);
        assert!(matches!(
            root_ref.doc.borrow().route_frame.frame,
            RouteFrameChoice::World(WorldRouteFrame {})
        ));
    });
}
