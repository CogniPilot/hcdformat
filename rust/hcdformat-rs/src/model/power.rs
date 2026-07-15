//! `<power-source>`: an energy source (battery/tank/fuel-cell/solar/supercapacitor). The container
//! plus the `<battery>` sub-tree (present in fixtures) are typed; the remaining source kinds are deep
//! and modeled loosely (leniently ignored on read) but their presence still parses.
use super::common::{MeasuredValue, RangeValue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "power-source")]
pub struct PowerSource {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@role", default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(rename = "@group", default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(rename = "@visual", default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<String>,
    #[serde(rename = "@mesh", default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<BatterySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tank: Option<TankSource>,
    #[serde(rename = "fuel-cell", default, skip_serializing_if = "Option::is_none")]
    pub fuel_cell: Option<FuelCell>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solar: Option<SolarSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supercapacitor: Option<SupercapacitorSource>,
}

/// `<battery>`: rechargeable battery energy source: cell config, chemistry, capacity, discharge
/// limits. Cell-count/chemistry text leaves are `String`; physical-quantity leaves are
/// [`MeasuredValue`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BatterySource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chemistry: Option<String>,
    #[serde(
        rename = "cells-series",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cells_series: Option<String>,
    #[serde(
        rename = "cells-parallel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cells_parallel: Option<String>,
    #[serde(
        rename = "nominal-voltage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nominal_voltage: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy: Option<MeasuredValue>,
    #[serde(
        rename = "max-discharge",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_discharge: Option<MeasuredValue>,
    #[serde(
        rename = "min-voltage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub min_voltage: Option<MeasuredValue>,
    #[serde(
        rename = "max-voltage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_voltage: Option<MeasuredValue>,
    #[serde(
        rename = "max-charge-rate",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_charge_rate: Option<MeasuredValue>,
    #[serde(
        rename = "charge-protocol",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub charge_protocol: Option<String>,
    #[serde(
        rename = "cycle-life",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cycle_life: Option<String>,
}

/// `<tank>` (`tank_source`, hcdf.xsd:1611): fuel/gas tank energy source: fuel type, volume, pressure,
/// stored energy, and a `<flow>` characterization. Numeric leaves are [`MeasuredValue`]; `fuel` is text.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TankSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<FlowCharacterization>,
}

/// `<flow>` (`flow_characterization`, hcdf.xsd:1590): how fluid exits a tank and at what rate.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FlowCharacterization {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(
        rename = "outlet-pressure",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub outlet_pressure: Option<RangeValue>,
    #[serde(
        rename = "max-flow-rate",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_flow_rate: Option<MeasuredValue>,
    #[serde(
        rename = "peak-flow-rate",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub peak_flow_rate: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regulator: Option<String>,
}

/// `<fuel-cell>` (`fuel_cell`, hcdf.xsd:1632): converts chemical fuel directly to electricity.
/// `efficiency` is an `xs:double` kept as `String` to preserve authored text (model convention).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FuelCell {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuel: Option<String>,
    #[serde(
        rename = "rated-power",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub rated_power: Option<MeasuredValue>,
    #[serde(
        rename = "peak-power",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub peak_power: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efficiency: Option<String>,
    #[serde(
        rename = "output-voltage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub output_voltage: Option<RangeValue>,
}

/// `<solar>` (`solar_source`, hcdf.xsd:1656): photovoltaic energy source. `efficiency` (`xs:double`)
/// and `mppt` (`xs:boolean`) are kept as `String` to preserve authored text verbatim.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SolarSource {
    #[serde(rename = "cell-type", default, skip_serializing_if = "Option::is_none")]
    pub cell_type: Option<String>,
    #[serde(
        rename = "peak-power",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub peak_power: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efficiency: Option<String>,
    #[serde(
        rename = "voltage-mpp",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub voltage_mpp: Option<MeasuredValue>,
    #[serde(
        rename = "current-mpp",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub current_mpp: Option<MeasuredValue>,
    #[serde(
        rename = "open-circuit-voltage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub open_circuit_voltage: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mppt: Option<String>,
}

/// `<supercapacitor>` (`supercapacitor_source`, hcdf.xsd:1686): electrostatic energy source.
/// `cells-series`/`cycle-life` (`xs:unsignedInt`) are kept as `String` to preserve authored text.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SupercapacitorSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacitance: Option<MeasuredValue>,
    #[serde(
        rename = "max-voltage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_voltage: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub esr: Option<MeasuredValue>,
    #[serde(
        rename = "max-current",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_current: Option<MeasuredValue>,
    #[serde(
        rename = "peak-current",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub peak_current: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy: Option<MeasuredValue>,
    #[serde(
        rename = "cells-series",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cells_series: Option<String>,
    #[serde(
        rename = "cycle-life",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cycle_life: Option<String>,
}
