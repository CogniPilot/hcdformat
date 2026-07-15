use hcdformat::model::{
    ComponentNamedFrame, ComponentOriginFrame, RouteFrame, RouteFrameChoice, WorldRouteFrame,
};
use hcdformat_pyderive::{pychoice, pydom};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

struct RouteDocument {
    route_frame: RouteFrame,
}

impl RouteDocument {
    fn from_xml_str(_xml: &str) -> Result<Self, String> {
        Ok(Self {
            route_frame: RouteFrame {
                frame: RouteFrameChoice::ComponentFrame(ComponentNamedFrame {
                    component: "initial".to_owned(),
                    frame: "initial".to_owned(),
                    instance: None,
                }),
            },
        })
    }

    fn to_xml_string(&self) -> Result<String, String> {
        Ok(String::new())
    }
}

fn draft_world() -> WorldRouteFrame {
    WorldRouteFrame {}
}

fn draft_component_frame() -> ComponentNamedFrame {
    ComponentNamedFrame {
        component: String::new(),
        frame: String::new(),
        instance: None,
    }
}

fn draft_component_origin() -> ComponentOriginFrame {
    ComponentOriginFrame {
        component: String::new(),
        instance: None,
    }
}

fn err(error: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(error.to_string())
}

fn stale(name: &str) -> PyErr {
    PyValueError::new_err(format!("stale handle: {name}"))
}

trait Resolve<Root>: Sized {
    type Loc;
    fn resolve<'a>(root: &'a Root, loc: &Self::Loc) -> Option<&'a Self>;
    fn resolve_mut<'a>(root: &'a mut Root, loc: &Self::Loc) -> Option<&'a mut Self>;
}

#[pydom(
    py = PyRouteDocument,
    target = RouteDocument,
    owner(target = RouteDocument, py = PyRouteDocument),
    root
)]
struct RouteDocumentDom {
    #[pydom(required_nested = PyRealRouteFrame)]
    route_frame: (),
}

#[pydom(
    py = PyRealRouteFrame,
    target = RouteFrame,
    owner(target = RouteDocument, py = PyRouteDocument),
    site(
        parent = PyRouteDocument,
        ptype = RouteDocument,
        field = route_frame,
        card = req
    ),
)]
struct RealRouteFrameDom {
    #[pydom(choice = PyRealRouteFrameChoice)]
    frame: (),
}

#[pydom(
    py = PyRealWorldRouteFrame,
    target = WorldRouteFrame,
    owner(target = RouteDocument, py = PyRouteDocument),
    choice_site(
        parent = PyRealRouteFrameChoice,
        ptype = RouteFrameChoice,
        variant = World
    ),
)]
struct RealWorldRouteFrameDom {}

#[pydom(
    py = PyRealComponentNamedFrame,
    target = ComponentNamedFrame,
    owner(target = RouteDocument, py = PyRouteDocument),
    choice_site(
        parent = PyRealRouteFrameChoice,
        ptype = RouteFrameChoice,
        variant = ComponentFrame
    ),
)]
struct RealComponentNamedFrameDom {
    component: String,
    frame: String,
}

#[pydom(
    py = PyRealComponentOriginFrame,
    target = ComponentOriginFrame,
    owner(target = RouteDocument, py = PyRouteDocument),
    choice_site(
        parent = PyRealRouteFrameChoice,
        ptype = RouteFrameChoice,
        variant = ComponentOrigin
    ),
)]
struct RealComponentOriginFrameDom {
    component: String,
}

#[pychoice(
    py = PyRealRouteFrameChoice,
    target = RouteFrameChoice,
    owner(target = RouteDocument, py = PyRouteDocument),
    site(
        parent = PyRealRouteFrame,
        ptype = RouteFrame,
        field = frame,
        card = req
    ),
)]
enum RealRouteFrameChoiceDom {
    #[pychoice(
        name = "world",
        payload = PyRealWorldRouteFrame,
        draft = draft_world
    )]
    World(WorldRouteFrame),
    #[pychoice(
        name = "component_origin",
        payload = PyRealComponentOriginFrame,
        draft = draft_component_origin
    )]
    ComponentOrigin(ComponentOriginFrame),
    #[pychoice(
        name = "component_frame",
        payload = PyRealComponentNamedFrame,
        draft = draft_component_frame
    )]
    ComponentFrame(ComponentNamedFrame),
}

#[test]
fn fresh_root_selects_actual_world_route_frame_arm() {
    Python::with_gil(|py| {
        let root = Py::new(py, PyRouteDocument::loads("<route/>").unwrap()).unwrap();
        let route = root.bind(py).getattr("route_frame").unwrap();
        let frame = route.getattr("frame").unwrap();
        let old_component = frame.call_method0("select_component_frame").unwrap();
        old_component.setattr("component", "arm").unwrap();

        frame.call_method0("select_world").unwrap();
        assert!(old_component.getattr("component").is_err());
        let root_ref = root.borrow(py);
        assert!(matches!(
            root_ref.doc.borrow().route_frame.frame,
            RouteFrameChoice::World(WorldRouteFrame {})
        ));
    });
}
