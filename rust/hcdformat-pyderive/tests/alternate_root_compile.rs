use hcdformat_pyderive::pydom;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[derive(Debug, Clone, PartialEq)]
struct StreamSettings {
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct StreamProfile {
    name: String,
    settings: StreamSettings,
}

impl StreamProfile {
    fn from_xml_str(_xml: &str) -> Result<Self, String> {
        Ok(Self {
            name: "loaded".to_owned(),
            settings: StreamSettings { enabled: true },
        })
    }

    fn to_xml_string(&self) -> Result<String, String> {
        Ok(format!("{}:{}", self.name, self.settings.enabled))
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
    py = PyStreamProfile,
    target = StreamProfile,
    owner(target = StreamProfile, py = PyStreamProfile),
    root
)]
struct StreamProfileDom {
    name: String,
    #[pydom(required_nested = PyStreamSettings)]
    settings: (),
}

#[pydom(
    py = PyStreamSettings,
    target = StreamSettings,
    owner(target = StreamProfile, py = PyStreamProfile),
    site(
        parent = PyStreamProfile,
        ptype = StreamProfile,
        field = settings,
        card = req
    ),
)]
struct StreamSettingsDom {
    #[pydom(required_scalar)]
    enabled: bool,
}

#[test]
fn differently_named_root_owns_and_resolves_its_children() {
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let root = Py::new(py, PyStreamProfile::loads("<stream-profile/>").unwrap()).unwrap();
        assert_eq!(
            root.bind(py)
                .call_method0("dumps")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "loaded:true"
        );

        let settings = root.bind(py).getattr("settings").unwrap();
        assert!(settings
            .getattr("enabled")
            .unwrap()
            .extract::<bool>()
            .unwrap());
        settings.setattr("enabled", false).unwrap();
        assert_eq!(
            root.bind(py)
                .call_method0("dumps")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "loaded:false"
        );
    });
}
