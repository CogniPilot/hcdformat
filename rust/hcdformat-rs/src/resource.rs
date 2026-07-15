//! Typed discovery and atomic rewriting of HCDF document and opaque asset resources.
//!
//! The walker covers resource-bearing fields in the authored crate::model::Hcdf tree. A
//! ResourceRewrite::Replace supplies both the URI and digest together, so callers cannot update one
//! half of a pinned reference while retaining the other half. Metadata URI fields, including firmware
//! manifest and mesh source-uri fields, are intentionally outside this walk. A connectivity
//! model-part points into another model root and is not a separate resource.

use crate::model::connectivity_xml::{
    Assembly, Connector, Junction, Path, Position, Representation, RepresentationChoice,
    Termination,
};
use crate::model::{Geometry, Hcdf, Sensor, VisualAppearance};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceClass {
    Document,
    OpaqueAsset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssemblyKind {
    Harness,
    Cable,
    Plumbing,
    Umbilical,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConnectivityContainer {
    Component {
        component_index: usize,
        component: String,
    },
    Assembly {
        kind: AssemblyKind,
        assembly_index: usize,
        assembly: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PositionKind {
    Pin,
    Socket,
    Contact,
    Fiber,
    Passage,
    Feed,
    WaveguideOpening,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathKind {
    Wire,
    Conductor,
    CableMember,
    Fiber,
    Coax,
    Waveguide,
    Feed,
    Hose,
    Pipe,
    Passage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JunctionKind {
    Splice,
    Tee,
    Manifold,
    Busbar,
    OpticalSplitter,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConnectivityRepresentationOwner {
    Connector {
        index: usize,
        name: String,
    },
    Position {
        connector_index: usize,
        connector: String,
        kind: PositionKind,
        index: usize,
        name: String,
    },
    Antenna {
        index: usize,
        name: String,
    },
    Path {
        kind: PathKind,
        index: usize,
        name: String,
    },
    Junction {
        kind: JunctionKind,
        index: usize,
        name: String,
    },
    Termination {
        index: usize,
        name: String,
    },
    Assembly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SensorGeometryKind {
    Inertial,
    Em,
    Optical,
    OpticalFov,
    Rf,
    Chemical,
    Force,
    Encoder,
    Temperature,
    Radiation,
    Audio,
    Tactile,
    Fluid,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceSite {
    IncludeDocument {
        index: usize,
    },
    ComponentVisualModel {
        component_index: usize,
        component: String,
        visual_index: usize,
        visual: String,
    },
    ComponentCollisionMesh {
        component_index: usize,
        component: String,
        collision_index: usize,
        collision: Option<String>,
    },
    SensorGeometryMesh {
        component_index: usize,
        component: String,
        sensor_index: usize,
        sensor: Option<String>,
        kind: SensorGeometryKind,
        category_index: usize,
        fov_index: Option<usize>,
    },
    HmiGeometryMesh {
        component_index: usize,
        component: String,
        hmi_index: usize,
        hmi: Option<String>,
    },
    ConnectivityModel {
        container: ConnectivityContainer,
        owner: ConnectivityRepresentationOwner,
    },
}

impl ResourceSite {
    pub fn class(&self) -> ResourceClass {
        match self {
            Self::IncludeDocument { .. } => ResourceClass::Document,
            _ => ResourceClass::OpaqueAsset,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceReference<'a> {
    pub uri: &'a str,
    pub sha: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRewrite {
    Keep,
    Replace { uri: String, sha: Option<String> },
}

/// Visits every active resource in stable authored order.
///
/// A replacement changes URI and SHA together. If the visitor returns an error, the current site is
/// unchanged. Replacements already accepted at earlier sites remain applied.
pub fn visit_resources<E, F>(doc: &mut Hcdf, mut visitor: F) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    for (index, include) in doc.include.iter_mut().enumerate() {
        visit_optional(
            &ResourceSite::IncludeDocument { index },
            &mut include.uri,
            &mut include.sha,
            &mut visitor,
        )?;
    }

    for (component_index, component) in doc.comp.iter_mut().enumerate() {
        let component_name = component.name.clone();
        for (visual_index, visual) in component.visual.iter_mut().enumerate() {
            if let VisualAppearance::Model { model, .. } = &mut visual.appearance {
                let site = ResourceSite::ComponentVisualModel {
                    component_index,
                    component: component_name.clone(),
                    visual_index,
                    visual: visual.name.clone(),
                };
                visit_optional(&site, &mut model.uri, &mut model.sha, &mut visitor)?;
            }
        }
        for (collision_index, collision) in component.collision.iter_mut().enumerate() {
            let Some(mesh) = collision
                .geometry
                .as_mut()
                .and_then(|geometry| geometry.mesh.as_mut())
            else {
                continue;
            };
            let site = ResourceSite::ComponentCollisionMesh {
                component_index,
                component: component_name.clone(),
                collision_index,
                collision: collision.name.clone(),
            };
            visit_optional(&site, &mut mesh.uri, &mut mesh.sha, &mut visitor)?;
        }

        for (sensor_index, sensor) in component.sensor.iter_mut().enumerate() {
            visit_sensor(
                component_index,
                &component_name,
                sensor_index,
                sensor,
                &mut visitor,
            )?;
        }
        for (hmi_index, hmi) in component.hmi.iter_mut().enumerate() {
            let site = ResourceSite::HmiGeometryMesh {
                component_index,
                component: component_name.clone(),
                hmi_index,
                hmi: hmi.name.clone(),
            };
            visit_geometry(&site, hmi.geometry.as_mut(), &mut visitor)?;
        }

        let container = ConnectivityContainer::Component {
            component_index,
            component: component_name,
        };
        visit_connectivity_container(
            &container,
            &mut component.connector,
            &mut component.antenna,
            &mut component.wire,
            &mut component.conductor,
            &mut component.cable_member,
            &mut component.fiber,
            &mut component.coax,
            &mut component.waveguide,
            &mut component.feed,
            &mut component.hose,
            &mut component.pipe,
            &mut component.passage,
            &mut component.splice,
            &mut component.tee,
            &mut component.manifold,
            &mut component.busbar,
            &mut component.optical_splitter,
            &mut component.termination,
            None,
            &mut visitor,
        )?;
    }

    visit_assemblies(AssemblyKind::Harness, &mut doc.harness, &mut visitor)?;
    visit_assemblies(AssemblyKind::Cable, &mut doc.cable, &mut visitor)?;
    visit_assemblies(AssemblyKind::Plumbing, &mut doc.plumbing, &mut visitor)?;
    visit_assemblies(AssemblyKind::Umbilical, &mut doc.umbilical, &mut visitor)?;
    Ok(())
}

fn visit_optional<E, F>(
    site: &ResourceSite,
    uri: &mut Option<String>,
    sha: &mut Option<String>,
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    let Some(current_uri) = uri.as_deref() else {
        return Ok(());
    };
    let action = visitor(
        site,
        ResourceReference {
            uri: current_uri,
            sha: sha.as_deref(),
        },
    )?;
    if let ResourceRewrite::Replace {
        uri: next_uri,
        sha: next_sha,
    } = action
    {
        *uri = Some(next_uri);
        *sha = next_sha;
    }
    Ok(())
}

fn visit_required<E, F>(
    site: &ResourceSite,
    uri: &mut String,
    sha: &mut Option<String>,
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    let action = visitor(
        site,
        ResourceReference {
            uri,
            sha: sha.as_deref(),
        },
    )?;
    if let ResourceRewrite::Replace {
        uri: next_uri,
        sha: next_sha,
    } = action
    {
        *uri = next_uri;
        *sha = next_sha;
    }
    Ok(())
}

fn visit_geometry<E, F>(
    site: &ResourceSite,
    geometry: Option<&mut Geometry>,
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    if let Some(mesh) = geometry.and_then(|geometry| geometry.mesh.as_mut()) {
        visit_optional(site, &mut mesh.uri, &mut mesh.sha, visitor)?;
    }
    Ok(())
}

fn sensor_site(
    component_index: usize,
    component: &str,
    sensor_index: usize,
    sensor: Option<String>,
    kind: SensorGeometryKind,
    category_index: usize,
    fov_index: Option<usize>,
) -> ResourceSite {
    ResourceSite::SensorGeometryMesh {
        component_index,
        component: component.to_owned(),
        sensor_index,
        sensor,
        kind,
        category_index,
        fov_index,
    }
}

fn visit_sensor<E, F>(
    component_index: usize,
    component: &str,
    sensor_index: usize,
    sensor: &mut Sensor,
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    let sensor_name = sensor.name.clone();
    macro_rules! category {
        ($field:ident, $kind:expr) => {
            for (category_index, category) in sensor.$field.iter_mut().enumerate() {
                let site = sensor_site(
                    component_index,
                    component,
                    sensor_index,
                    sensor_name.clone(),
                    $kind,
                    category_index,
                    None,
                );
                visit_geometry(&site, category.geometry.as_mut(), visitor)?;
            }
        };
    }
    category!(inertial, SensorGeometryKind::Inertial);
    category!(em, SensorGeometryKind::Em);
    for (category_index, optical) in sensor.optical.iter_mut().enumerate() {
        let site = sensor_site(
            component_index,
            component,
            sensor_index,
            sensor_name.clone(),
            SensorGeometryKind::Optical,
            category_index,
            None,
        );
        visit_geometry(&site, optical.geometry.as_mut(), visitor)?;
        for (fov_index, fov) in optical.fov.iter_mut().enumerate() {
            let site = sensor_site(
                component_index,
                component,
                sensor_index,
                sensor_name.clone(),
                SensorGeometryKind::OpticalFov,
                category_index,
                Some(fov_index),
            );
            visit_geometry(&site, fov.geometry.as_mut(), visitor)?;
        }
    }
    category!(rf, SensorGeometryKind::Rf);
    category!(chemical, SensorGeometryKind::Chemical);
    category!(force, SensorGeometryKind::Force);
    category!(encoder, SensorGeometryKind::Encoder);
    category!(temperature, SensorGeometryKind::Temperature);
    category!(radiation, SensorGeometryKind::Radiation);
    category!(audio, SensorGeometryKind::Audio);
    category!(tactile, SensorGeometryKind::Tactile);
    category!(fluid, SensorGeometryKind::Fluid);
    Ok(())
}

fn visit_representation<E, F>(
    container: &ConnectivityContainer,
    owner: ConnectivityRepresentationOwner,
    representation: Option<&mut Representation>,
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    let Some(RepresentationChoice::Model(model)) =
        representation.map(|representation| &mut representation.variant)
    else {
        return Ok(());
    };
    let site = ResourceSite::ConnectivityModel {
        container: container.clone(),
        owner,
    };
    visit_required(&site, &mut model.uri, &mut model.sha, visitor)
}

fn visit_positions<E, F>(
    container: &ConnectivityContainer,
    connector_index: usize,
    connector: &str,
    kind: PositionKind,
    positions: &mut [Position],
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    for (index, position) in positions.iter_mut().enumerate() {
        visit_representation(
            container,
            ConnectivityRepresentationOwner::Position {
                connector_index,
                connector: connector.to_owned(),
                kind,
                index,
                name: position.name.clone(),
            },
            position.representation.as_mut(),
            visitor,
        )?;
    }
    Ok(())
}

fn visit_connectors<E, F>(
    container: &ConnectivityContainer,
    connectors: &mut [Connector],
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    for (index, connector) in connectors.iter_mut().enumerate() {
        let name = connector.name.clone();
        visit_representation(
            container,
            ConnectivityRepresentationOwner::Connector {
                index,
                name: name.clone(),
            },
            connector.representation.as_mut(),
            visitor,
        )?;
        visit_positions(
            container,
            index,
            &name,
            PositionKind::Pin,
            &mut connector.pin,
            visitor,
        )?;
        visit_positions(
            container,
            index,
            &name,
            PositionKind::Socket,
            &mut connector.socket,
            visitor,
        )?;
        visit_positions(
            container,
            index,
            &name,
            PositionKind::Contact,
            &mut connector.contact,
            visitor,
        )?;
        visit_positions(
            container,
            index,
            &name,
            PositionKind::Fiber,
            &mut connector.fiber,
            visitor,
        )?;
        visit_positions(
            container,
            index,
            &name,
            PositionKind::Passage,
            &mut connector.passage,
            visitor,
        )?;
        visit_positions(
            container,
            index,
            &name,
            PositionKind::Feed,
            &mut connector.feed,
            visitor,
        )?;
        visit_positions(
            container,
            index,
            &name,
            PositionKind::WaveguideOpening,
            &mut connector.waveguide_opening,
            visitor,
        )?;
    }
    Ok(())
}

fn visit_paths<E, F>(
    container: &ConnectivityContainer,
    kind: PathKind,
    paths: &mut [Path],
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    for (index, path) in paths.iter_mut().enumerate() {
        visit_representation(
            container,
            ConnectivityRepresentationOwner::Path {
                kind,
                index,
                name: path.name.clone(),
            },
            path.representation.as_mut(),
            visitor,
        )?;
    }
    Ok(())
}

fn visit_junctions<E, F>(
    container: &ConnectivityContainer,
    kind: JunctionKind,
    junctions: &mut [Junction],
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    for (index, junction) in junctions.iter_mut().enumerate() {
        visit_representation(
            container,
            ConnectivityRepresentationOwner::Junction {
                kind,
                index,
                name: junction.name.clone(),
            },
            junction.representation.as_mut(),
            visitor,
        )?;
    }
    Ok(())
}

fn visit_terminations<E, F>(
    container: &ConnectivityContainer,
    terminations: &mut [Termination],
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    for (index, termination) in terminations.iter_mut().enumerate() {
        visit_representation(
            container,
            ConnectivityRepresentationOwner::Termination {
                index,
                name: termination.name.clone(),
            },
            termination.representation.as_mut(),
            visitor,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn visit_connectivity_container<E, F>(
    container: &ConnectivityContainer,
    connectors: &mut [Connector],
    antennas: &mut [crate::model::connectivity_xml::Antenna],
    wire: &mut [Path],
    conductor: &mut [Path],
    cable_member: &mut [Path],
    fiber: &mut [Path],
    coax: &mut [Path],
    waveguide: &mut [Path],
    feed: &mut [Path],
    hose: &mut [Path],
    pipe: &mut [Path],
    passage: &mut [Path],
    splice: &mut [Junction],
    tee: &mut [Junction],
    manifold: &mut [Junction],
    busbar: &mut [Junction],
    optical_splitter: &mut [Junction],
    termination: &mut [Termination],
    assembly_representation: Option<&mut Representation>,
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    visit_connectors(container, connectors, visitor)?;
    for (index, antenna) in antennas.iter_mut().enumerate() {
        visit_representation(
            container,
            ConnectivityRepresentationOwner::Antenna {
                index,
                name: antenna.name.clone(),
            },
            antenna.representation.as_mut(),
            visitor,
        )?;
    }
    visit_paths(container, PathKind::Wire, wire, visitor)?;
    visit_paths(container, PathKind::Conductor, conductor, visitor)?;
    visit_paths(container, PathKind::CableMember, cable_member, visitor)?;
    visit_paths(container, PathKind::Fiber, fiber, visitor)?;
    visit_paths(container, PathKind::Coax, coax, visitor)?;
    visit_paths(container, PathKind::Waveguide, waveguide, visitor)?;
    visit_paths(container, PathKind::Feed, feed, visitor)?;
    visit_paths(container, PathKind::Hose, hose, visitor)?;
    visit_paths(container, PathKind::Pipe, pipe, visitor)?;
    visit_paths(container, PathKind::Passage, passage, visitor)?;
    visit_junctions(container, JunctionKind::Splice, splice, visitor)?;
    visit_junctions(container, JunctionKind::Tee, tee, visitor)?;
    visit_junctions(container, JunctionKind::Manifold, manifold, visitor)?;
    visit_junctions(container, JunctionKind::Busbar, busbar, visitor)?;
    visit_junctions(
        container,
        JunctionKind::OpticalSplitter,
        optical_splitter,
        visitor,
    )?;
    visit_terminations(container, termination, visitor)?;
    visit_representation(
        container,
        ConnectivityRepresentationOwner::Assembly,
        assembly_representation,
        visitor,
    )?;
    Ok(())
}

fn visit_assemblies<E, F>(
    kind: AssemblyKind,
    assemblies: &mut [Assembly],
    visitor: &mut F,
) -> Result<(), E>
where
    F: FnMut(&ResourceSite, ResourceReference<'_>) -> Result<ResourceRewrite, E>,
{
    for (assembly_index, assembly) in assemblies.iter_mut().enumerate() {
        let container = ConnectivityContainer::Assembly {
            kind,
            assembly_index,
            assembly: assembly.name.clone(),
        };
        visit_connectivity_container(
            &container,
            &mut assembly.connector,
            &mut [],
            &mut assembly.wire,
            &mut assembly.conductor,
            &mut assembly.cable_member,
            &mut assembly.fiber,
            &mut assembly.coax,
            &mut assembly.waveguide,
            &mut assembly.feed,
            &mut assembly.hose,
            &mut assembly.pipe,
            &mut assembly.passage,
            &mut assembly.splice,
            &mut assembly.tee,
            &mut assembly.manifold,
            &mut assembly.busbar,
            &mut assembly.optical_splitter,
            &mut assembly.termination,
            assembly.representation.as_mut(),
            visitor,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::connectivity_xml::{
        Placement, PlacementRotation, PlacementRotationChoice, RouteFrame, RouteFrameChoice,
        RpyRotation, WorldRouteFrame,
    };
    use crate::model::{Collision, CollisionGeometry, Comp, Include, Mesh, ModelRef, Visual};

    fn placement() -> Placement {
        Placement {
            xyz: [0.0; 3],
            frame: RouteFrame {
                frame: RouteFrameChoice::World(WorldRouteFrame {}),
            },
            rotation: PlacementRotation {
                rotation: PlacementRotationChoice::Rpy(RpyRotation { value: [0.0; 3] }),
            },
        }
    }

    fn connectivity_model(uri: &str) -> Representation {
        Representation {
            variant: RepresentationChoice::Model(
                crate::model::connectivity_xml::ModelRepresentation {
                    uri: uri.to_owned(),
                    sha: Some("old".to_owned()),
                    node_path: None,
                    placement: placement(),
                },
            ),
        }
    }

    #[test]
    fn visits_and_atomically_rewrites_resource_families() {
        let mut doc = Hcdf {
            include: vec![Include {
                uri: Some("/mem/0/module.hcdf".to_owned()),
                sha: Some("module-old".to_owned()),
                ..Default::default()
            }],
            comp: vec![Comp {
                name: "body".to_owned(),
                visual: vec![Visual {
                    name: "shell".to_owned(),
                    appearance: VisualAppearance::Model {
                        model: ModelRef {
                            uri: Some("shell.glb".to_owned()),
                            sha: Some("shell-old".to_owned()),
                            ..Default::default()
                        },
                        geometry: None,
                    },
                    ..Default::default()
                }],
                collision: vec![Collision {
                    geometry: Some(CollisionGeometry {
                        mesh: Some(Mesh {
                            uri: Some("collision.stl".to_owned()),
                            sha: Some("collision-old".to_owned()),
                            source_uri: Some("do-not-visit.obj".to_owned()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                connector: vec![Connector {
                    name: "J1".to_owned(),
                    representation: Some(connectivity_model("connector.glb")),
                    pin: vec![Position {
                        name: "1".to_owned(),
                        representation: Some(connectivity_model("pin.glb")),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            harness: vec![Assembly {
                name: "loom".to_owned(),
                representation: Some(connectivity_model("loom.glb")),
                ..Default::default()
            }],
            ..Default::default()
        };

        let mut visited = Vec::new();
        visit_resources(&mut doc, |site, reference| {
            visited.push((site.clone(), reference.uri.to_owned()));
            Ok::<_, ()>(ResourceRewrite::Replace {
                uri: format!("resolved:{}", reference.uri),
                sha: Some(format!("sha:{}", reference.uri)),
            })
        })
        .unwrap();

        assert_eq!(visited.len(), 6);
        assert_eq!(
            doc.include[0].uri.as_deref(),
            Some("resolved:/mem/0/module.hcdf")
        );
        assert_eq!(
            doc.include[0].sha.as_deref(),
            Some("sha:/mem/0/module.hcdf")
        );
        assert_eq!(
            doc.comp[0].collision[0]
                .geometry
                .as_ref()
                .unwrap()
                .mesh
                .as_ref()
                .unwrap()
                .source_uri
                .as_deref(),
            Some("do-not-visit.obj")
        );
    }

    #[test]
    fn keep_preserves_absolute_memory_keys_byte_for_byte() {
        let mut doc = Hcdf {
            include: vec![crate::model::Include {
                uri: Some("/mem/0/assets/rotor.stl".to_owned()),
                sha: Some("sha256:abc".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        visit_resources(&mut doc, |_site, reference| {
            assert_eq!(reference.uri, "/mem/0/assets/rotor.stl");
            Ok::<_, ()>(ResourceRewrite::Keep)
        })
        .unwrap();
        assert_eq!(
            doc.include[0].uri.as_deref(),
            Some("/mem/0/assets/rotor.stl")
        );
    }
}
