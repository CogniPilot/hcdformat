//! Closed unit vocabulary and dimension-aware normalization for connectivity quantities.
//!
//! Authored quantities retain their original value and unit. This module produces a separate
//! normalized value for validation and comparison.

use crate::model::connectivity::Quantity;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dimension {
    BitRate,
    SymbolRate,
    Frequency,
    Voltage,
    Current,
    Power,
    Impedance,
    Capacitance,
    Inductance,
    Pressure,
    VolumetricFlow,
    MassFlow,
    Temperature,
    Length,
    Duration,
    Angle,
    Ratio,
    Loss,
    IsotropicGain,
}

impl Dimension {
    pub fn name(self) -> &'static str {
        match self {
            Self::BitRate => "bit rate",
            Self::SymbolRate => "symbol rate",
            Self::Frequency => "frequency",
            Self::Voltage => "voltage",
            Self::Current => "current",
            Self::Power => "power",
            Self::Impedance => "impedance",
            Self::Capacitance => "capacitance",
            Self::Inductance => "inductance",
            Self::Pressure => "pressure",
            Self::VolumetricFlow => "volumetric flow",
            Self::MassFlow => "mass flow",
            Self::Temperature => "temperature",
            Self::Length => "length",
            Self::Duration => "duration",
            Self::Angle => "angle",
            Self::Ratio => "ratio",
            Self::Loss => "loss",
            Self::IsotropicGain => "isotropic gain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedQuantity {
    pub dimension: Dimension,
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantityErrorKind {
    Value,
    Unit,
    UnitUnknown,
    PhysicalRange,
}

impl QuantityErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Value => "E_CONN_QUANTITY_VALUE",
            Self::Unit => "E_CONN_UNIT",
            Self::UnitUnknown => "E_CONN_UNIT_UNKNOWN",
            Self::PhysicalRange => "E_CONN_PHYSICAL_RANGE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantityError {
    kind: QuantityErrorKind,
    message: String,
}

impl QuantityError {
    fn new(kind: QuantityErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> QuantityErrorKind {
        self.kind
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for QuantityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for QuantityError {}

#[derive(Clone, Copy)]
struct UnitDefinition {
    dimension: Dimension,
    scale: f64,
    offset: f64,
}

impl UnitDefinition {
    const fn linear(dimension: Dimension, scale: f64) -> Self {
        Self {
            dimension,
            scale,
            offset: 0.0,
        }
    }

    const fn affine(dimension: Dimension, scale: f64, offset: f64) -> Self {
        Self {
            dimension,
            scale,
            offset,
        }
    }
}

pub fn normalize_quantity(quantity: &Quantity) -> Result<NormalizedQuantity, QuantityError> {
    if !quantity.value.is_finite() {
        return Err(QuantityError::new(
            QuantityErrorKind::Value,
            "quantity value must be finite",
        ));
    }
    validate_unit_lexeme(&quantity.unit)?;
    let definition = unit_definition(&quantity.unit).ok_or_else(|| {
        QuantityError::new(
            QuantityErrorKind::UnitUnknown,
            format!("unknown connectivity unit {:?}", quantity.unit),
        )
    })?;
    let temperature_lower_bound = match quantity.unit.as_str() {
        "K" => Some(0.0),
        "degC" => Some(-273.15),
        "degF" => Some(-459.67),
        _ => None,
    };
    let temperature_at_lower_bound = if let Some(lower_bound) = temperature_lower_bound {
        if definitely_less(quantity.value, lower_bound) {
            return Err(QuantityError::new(
                QuantityErrorKind::PhysicalRange,
                "temperature must not be below absolute zero",
            ));
        }
        ulp_eq(quantity.value, lower_bound)
    } else {
        false
    };
    let mut value = if temperature_at_lower_bound {
        0.0
    } else {
        quantity.value.mul_add(definition.scale, definition.offset)
    };
    if !value.is_finite() {
        return Err(QuantityError::new(
            QuantityErrorKind::Value,
            "normalized quantity value must be finite",
        ));
    }
    let physical_range_message = match definition.dimension {
        Dimension::Temperature => Some("temperature must not be below absolute zero"),
        Dimension::BitRate
        | Dimension::SymbolRate
        | Dimension::Frequency
        | Dimension::Length
        | Dimension::Duration => Some("quantity dimension requires a nonnegative value"),
        _ => None,
    };
    if value < 0.0 {
        if let Some(message) = physical_range_message {
            if ulp_eq(value, 0.0) {
                value = 0.0;
            } else {
                return Err(QuantityError::new(
                    QuantityErrorKind::PhysicalRange,
                    message,
                ));
            }
        }
    }
    Ok(NormalizedQuantity {
        dimension: definition.dimension,
        value,
    })
}

fn validate_unit_lexeme(unit: &str) -> Result<(), QuantityError> {
    if unit.is_empty()
        || !unit.is_ascii()
        || unit.bytes().any(|byte| byte.is_ascii_whitespace())
        || !unit
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'/')
    {
        return Err(QuantityError::new(
            QuantityErrorKind::Unit,
            "unit must be a nonempty ASCII token without whitespace",
        ));
    }
    Ok(())
}

fn unit_definition(unit: &str) -> Option<UnitDefinition> {
    use Dimension::*;
    let definition = match unit {
        "bit/s" | "bps" => UnitDefinition::linear(BitRate, 1.0),
        "kbit/s" | "kbps" => UnitDefinition::linear(BitRate, 1.0e3),
        "Mbit/s" | "Mbps" => UnitDefinition::linear(BitRate, 1.0e6),
        "Gbit/s" | "Gbps" => UnitDefinition::linear(BitRate, 1.0e9),
        "Bd" => UnitDefinition::linear(SymbolRate, 1.0),
        "kBd" => UnitDefinition::linear(SymbolRate, 1.0e3),
        "MBd" => UnitDefinition::linear(SymbolRate, 1.0e6),
        "GBd" => UnitDefinition::linear(SymbolRate, 1.0e9),
        "Hz" => UnitDefinition::linear(Frequency, 1.0),
        "kHz" => UnitDefinition::linear(Frequency, 1.0e3),
        "MHz" => UnitDefinition::linear(Frequency, 1.0e6),
        "GHz" => UnitDefinition::linear(Frequency, 1.0e9),
        "THz" => UnitDefinition::linear(Frequency, 1.0e12),
        "uV" => UnitDefinition::linear(Voltage, 1.0e-6),
        "mV" => UnitDefinition::linear(Voltage, 1.0e-3),
        "V" => UnitDefinition::linear(Voltage, 1.0),
        "kV" => UnitDefinition::linear(Voltage, 1.0e3),
        "uA" => UnitDefinition::linear(Current, 1.0e-6),
        "mA" => UnitDefinition::linear(Current, 1.0e-3),
        "A" => UnitDefinition::linear(Current, 1.0),
        "kA" => UnitDefinition::linear(Current, 1.0e3),
        "uW" => UnitDefinition::linear(Power, 1.0e-6),
        "mW" => UnitDefinition::linear(Power, 1.0e-3),
        "W" => UnitDefinition::linear(Power, 1.0),
        "kW" => UnitDefinition::linear(Power, 1.0e3),
        "MW" => UnitDefinition::linear(Power, 1.0e6),
        "mohm" => UnitDefinition::linear(Impedance, 1.0e-3),
        "ohm" => UnitDefinition::linear(Impedance, 1.0),
        "kohm" => UnitDefinition::linear(Impedance, 1.0e3),
        "Mohm" => UnitDefinition::linear(Impedance, 1.0e6),
        "F" => UnitDefinition::linear(Capacitance, 1.0),
        "mF" => UnitDefinition::linear(Capacitance, 1.0e-3),
        "uF" => UnitDefinition::linear(Capacitance, 1.0e-6),
        "nF" => UnitDefinition::linear(Capacitance, 1.0e-9),
        "pF" => UnitDefinition::linear(Capacitance, 1.0e-12),
        "H" => UnitDefinition::linear(Inductance, 1.0),
        "mH" => UnitDefinition::linear(Inductance, 1.0e-3),
        "uH" => UnitDefinition::linear(Inductance, 1.0e-6),
        "nH" => UnitDefinition::linear(Inductance, 1.0e-9),
        "Pa" => UnitDefinition::linear(Pressure, 1.0),
        "hPa" => UnitDefinition::linear(Pressure, 1.0e2),
        "kPa" => UnitDefinition::linear(Pressure, 1.0e3),
        "MPa" => UnitDefinition::linear(Pressure, 1.0e6),
        "bar" => UnitDefinition::linear(Pressure, 1.0e5),
        "mbar" => UnitDefinition::linear(Pressure, 1.0e2),
        "psi" => UnitDefinition::linear(Pressure, 6_894.757_293_168),
        "m3/s" => UnitDefinition::linear(VolumetricFlow, 1.0),
        "L/s" => UnitDefinition::linear(VolumetricFlow, 1.0e-3),
        "L/min" => UnitDefinition::linear(VolumetricFlow, 1.0e-3 / 60.0),
        "mL/s" => UnitDefinition::linear(VolumetricFlow, 1.0e-6),
        "mL/min" => UnitDefinition::linear(VolumetricFlow, 1.0e-6 / 60.0),
        "kg/s" => UnitDefinition::linear(MassFlow, 1.0),
        "g/s" => UnitDefinition::linear(MassFlow, 1.0e-3),
        "kg/min" => UnitDefinition::linear(MassFlow, 1.0 / 60.0),
        "K" => UnitDefinition::linear(Temperature, 1.0),
        "degC" => UnitDefinition::affine(Temperature, 1.0, 273.15),
        "degF" => UnitDefinition::affine(Temperature, 5.0 / 9.0, 255.372_222_222_222_2),
        "m" => UnitDefinition::linear(Length, 1.0),
        "mm" => UnitDefinition::linear(Length, 1.0e-3),
        "um" => UnitDefinition::linear(Length, 1.0e-6),
        "nm" => UnitDefinition::linear(Length, 1.0e-9),
        "in" => UnitDefinition::linear(Length, 0.0254),
        "s" => UnitDefinition::linear(Duration, 1.0),
        "ms" => UnitDefinition::linear(Duration, 1.0e-3),
        "us" => UnitDefinition::linear(Duration, 1.0e-6),
        "ns" => UnitDefinition::linear(Duration, 1.0e-9),
        "rad" => UnitDefinition::linear(Angle, 1.0),
        "deg" => UnitDefinition::linear(Angle, std::f64::consts::PI / 180.0),
        "1" => UnitDefinition::linear(Ratio, 1.0),
        "percent" => UnitDefinition::linear(Ratio, 0.01),
        "dB" => UnitDefinition::linear(Loss, 1.0),
        "dBi" => UnitDefinition::linear(IsotropicGain, 1.0),
        _ => return None,
    };
    Some(definition)
}

const ULP_TOLERANCE: u64 = 8;

pub fn ulp_eq(first: f64, second: f64) -> bool {
    if first == second {
        return true;
    }
    if !first.is_finite() || !second.is_finite() {
        return false;
    }
    ordered_bits(first).abs_diff(ordered_bits(second)) <= ULP_TOLERANCE
}

const TEMPERATURE_ABSOLUTE_TOLERANCE_K: f64 = 1.0e-12;

pub fn normalized_eq(first: NormalizedQuantity, second: NormalizedQuantity) -> bool {
    if first.dimension != second.dimension {
        return false;
    }
    ulp_eq(first.value, second.value)
        || first.dimension == Dimension::Temperature
            && (first.value - second.value).abs() <= TEMPERATURE_ABSOLUTE_TOLERANCE_K
}

pub fn definitely_less_normalized(first: NormalizedQuantity, second: NormalizedQuantity) -> bool {
    first.dimension == second.dimension
        && first.value < second.value
        && !normalized_eq(first, second)
}

pub fn definitely_greater_normalized(
    first: NormalizedQuantity,
    second: NormalizedQuantity,
) -> bool {
    first.dimension == second.dimension
        && first.value > second.value
        && !normalized_eq(first, second)
}

pub fn definitely_less(first: f64, second: f64) -> bool {
    first < second && !ulp_eq(first, second)
}

pub fn definitely_greater(first: f64, second: f64) -> bool {
    first > second && !ulp_eq(first, second)
}

fn ordered_bits(value: f64) -> u64 {
    let bits = value.to_bits();
    if bits & (1 << 63) == 0 {
        bits | (1 << 63)
    } else {
        !bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantity(value: f64, unit: &str) -> Quantity {
        Quantity {
            value,
            unit: unit.to_owned(),
        }
    }

    #[test]
    fn every_approved_unit_spelling_is_accepted_with_its_exact_dimension() {
        use Dimension::*;
        let cases = [
            ("bit/s", BitRate),
            ("bps", BitRate),
            ("kbit/s", BitRate),
            ("kbps", BitRate),
            ("Mbit/s", BitRate),
            ("Mbps", BitRate),
            ("Gbit/s", BitRate),
            ("Gbps", BitRate),
            ("Bd", SymbolRate),
            ("kBd", SymbolRate),
            ("MBd", SymbolRate),
            ("GBd", SymbolRate),
            ("Hz", Frequency),
            ("kHz", Frequency),
            ("MHz", Frequency),
            ("GHz", Frequency),
            ("THz", Frequency),
            ("uV", Voltage),
            ("mV", Voltage),
            ("V", Voltage),
            ("kV", Voltage),
            ("uA", Current),
            ("mA", Current),
            ("A", Current),
            ("kA", Current),
            ("uW", Power),
            ("mW", Power),
            ("W", Power),
            ("kW", Power),
            ("MW", Power),
            ("mohm", Impedance),
            ("ohm", Impedance),
            ("kohm", Impedance),
            ("Mohm", Impedance),
            ("F", Capacitance),
            ("mF", Capacitance),
            ("uF", Capacitance),
            ("nF", Capacitance),
            ("pF", Capacitance),
            ("H", Inductance),
            ("mH", Inductance),
            ("uH", Inductance),
            ("nH", Inductance),
            ("Pa", Pressure),
            ("hPa", Pressure),
            ("kPa", Pressure),
            ("MPa", Pressure),
            ("bar", Pressure),
            ("mbar", Pressure),
            ("psi", Pressure),
            ("m3/s", VolumetricFlow),
            ("L/s", VolumetricFlow),
            ("L/min", VolumetricFlow),
            ("mL/s", VolumetricFlow),
            ("mL/min", VolumetricFlow),
            ("kg/s", MassFlow),
            ("g/s", MassFlow),
            ("kg/min", MassFlow),
            ("K", Temperature),
            ("degC", Temperature),
            ("degF", Temperature),
            ("m", Length),
            ("mm", Length),
            ("um", Length),
            ("nm", Length),
            ("in", Length),
            ("s", Duration),
            ("ms", Duration),
            ("us", Duration),
            ("ns", Duration),
            ("rad", Angle),
            ("deg", Angle),
            ("1", Ratio),
            ("percent", Ratio),
            ("dB", Loss),
            ("dBi", IsotropicGain),
        ];
        for (unit, dimension) in cases {
            assert_eq!(
                normalize_quantity(&quantity(1.0, unit)).unwrap().dimension,
                dimension,
                "unexpected dimension for {unit}"
            );
        }
    }
    #[test]
    fn equivalent_units_normalize_to_one_dimension() {
        let megabits = normalize_quantity(&quantity(1.0, "Mbit/s")).unwrap();
        let bits = normalize_quantity(&quantity(1_000_000.0, "bit/s")).unwrap();
        assert_eq!(megabits.dimension, Dimension::BitRate);
        assert!(ulp_eq(megabits.value, bits.value));
    }

    #[test]
    fn passive_component_units_normalize_across_prefixes() {
        let microfarad = normalize_quantity(&quantity(1.0, "uF")).unwrap();
        let nanofarads = normalize_quantity(&quantity(1_000.0, "nF")).unwrap();
        assert_eq!(microfarad.dimension, Dimension::Capacitance);
        assert!(ulp_eq(microfarad.value, nanofarads.value));

        let millihenry = normalize_quantity(&quantity(1.0, "mH")).unwrap();
        let microhenries = normalize_quantity(&quantity(1_000.0, "uH")).unwrap();
        assert_eq!(millihenry.dimension, Dimension::Inductance);
        assert!(ulp_eq(millihenry.value, microhenries.value));
    }

    #[test]
    fn distinct_semantics_remain_distinct_dimensions() {
        assert_ne!(
            normalize_quantity(&quantity(1.0, "Bd")).unwrap().dimension,
            normalize_quantity(&quantity(1.0, "Hz")).unwrap().dimension
        );
        assert_ne!(
            normalize_quantity(&quantity(1.0, "dB")).unwrap().dimension,
            normalize_quantity(&quantity(1.0, "dBi")).unwrap().dimension
        );
    }

    #[test]
    fn affine_temperature_units_normalize() {
        let freezing_c = normalize_quantity(&quantity(0.0, "degC")).unwrap();
        let freezing_f = normalize_quantity(&quantity(32.0, "degF")).unwrap();
        assert!(ulp_eq(freezing_c.value, 273.15));
        assert!(ulp_eq(freezing_c.value, freezing_f.value));
    }

    #[test]
    fn affine_temperature_comparison_covers_near_zero_conversion_cancellation() {
        let celsius = normalize_quantity(&quantity(-273.149, "degC")).unwrap();
        let fahrenheit = normalize_quantity(&quantity(-459.6682, "degF")).unwrap();
        assert!(!ulp_eq(celsius.value, fahrenheit.value));
        assert!(normalized_eq(celsius, fahrenheit));
        assert!(!definitely_less_normalized(celsius, fahrenheit));
        assert!(!definitely_greater_normalized(celsius, fahrenheit));
    }

    #[test]
    fn normalized_ordering_requires_matching_dimensions() {
        let voltage = normalize_quantity(&quantity(1.0, "V")).unwrap();
        let frequency = normalize_quantity(&quantity(2.0, "Hz")).unwrap();
        assert!(!definitely_less_normalized(voltage, frequency));
        assert!(!definitely_greater_normalized(voltage, frequency));
        assert!(!definitely_less_normalized(frequency, voltage));
        assert!(!definitely_greater_normalized(frequency, voltage));
    }

    #[test]
    fn absolute_zero_is_checked_in_each_authored_temperature_unit() {
        for (unit, lower_bound) in [("K", 0.0_f64), ("degC", -273.15), ("degF", -459.67)] {
            assert_eq!(
                normalize_quantity(&quantity(lower_bound, unit))
                    .unwrap()
                    .value,
                0.0,
                "exact absolute zero failed for {unit}"
            );
            let tolerated_below = if lower_bound == 0.0 {
                -f64::from_bits(1)
            } else {
                f64::from_bits(lower_bound.to_bits() + 8)
            };
            assert_eq!(
                normalize_quantity(&quantity(tolerated_below, unit))
                    .unwrap()
                    .value,
                0.0,
                "ULP-tolerated absolute zero failed for {unit}"
            );
            let below = if lower_bound == 0.0 {
                -f64::from_bits(16)
            } else {
                f64::from_bits(lower_bound.to_bits() + 9)
            };
            assert_eq!(
                normalize_quantity(&quantity(below, unit))
                    .unwrap_err()
                    .code(),
                "E_CONN_PHYSICAL_RANGE",
                "below absolute zero was accepted for {unit}"
            );
        }
    }

    #[test]
    fn rejects_invalid_units_values_and_absolute_zero() {
        for unit in [" MHz", "MHz ", "M Hz", "uV\t", "µV", "µF", "μH"] {
            assert_eq!(
                normalize_quantity(&quantity(1.0, unit)).unwrap_err().code(),
                "E_CONN_UNIT",
                "unexpected classification for {unit:?}"
            );
        }
        for unit in ["mhz", "BPS", "db", "DBI", "f", "UF", "mh", "NH"] {
            assert_eq!(
                normalize_quantity(&quantity(1.0, unit)).unwrap_err().code(),
                "E_CONN_UNIT_UNKNOWN",
                "unexpected classification for {unit:?}"
            );
        }
        assert_eq!(
            normalize_quantity(&quantity(f64::INFINITY, "Hz"))
                .unwrap_err()
                .code(),
            "E_CONN_QUANTITY_VALUE"
        );
        assert_eq!(
            normalize_quantity(&quantity(-1.0, "K")).unwrap_err().code(),
            "E_CONN_PHYSICAL_RANGE"
        );
    }

    #[test]
    fn intrinsically_nonnegative_dimensions_reject_negative_values_with_ulp_tolerance() {
        for unit in ["bit/s", "Bd", "Hz", "m", "s"] {
            assert_eq!(
                normalize_quantity(&quantity(-1.0, unit))
                    .unwrap_err()
                    .code(),
                "E_CONN_PHYSICAL_RANGE",
                "negative {unit} must be rejected"
            );
        }
        let tiny_negative = normalize_quantity(&quantity(-f64::from_bits(1), "Hz")).unwrap();
        assert_eq!(tiny_negative.value, 0.0);
        assert_eq!(
            normalize_quantity(&quantity(-f64::from_bits(16), "Hz"))
                .unwrap_err()
                .code(),
            "E_CONN_PHYSICAL_RANGE"
        );
    }
}
