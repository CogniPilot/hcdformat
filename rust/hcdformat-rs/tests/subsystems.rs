//! Model-completion coverage: the leaf subsystems that the hand-written model previously dropped
//! (radar/lidar/gnss/camera params, camera intrinsics, data-output, IMU fifo, battery, prop, motor
//! thrust, connectivity ports/functions/antennas, network selections, topology, and configuration)
//! now parse into typed fields AND round-trip without loss. The live-oracle
//! `parity` harness (allowlist now empty) proves no element is dropped corpus-wide; these unit tests pin
//! the specific field shapes so a future retype/rename regression is caught with a precise message.
use hcdformat::Hcdf;

/// Parse `src`, assert it equals its own serialize/parse fixpoint, and return the parsed document.
fn roundtrip(src: &str) -> Hcdf {
    let doc = Hcdf::from_xml_str(src).expect("parse");
    let out = doc.to_xml_string().expect("serialize");
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "round-trip mismatch; serialized = {out}");
    doc
}

#[test]
fn optical_camera_lidar_params_and_intrinsics_roundtrip() {
    let doc = roundtrip(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <sensor name="cam">
            <optical type="camera">
              <fov name="main"><intrinsics><width>1920</width><height>1080</height>
                <format>RGB8</format><fx>1000</fx><fy>1000</fy><cx>960</cx><cy>540</cy></intrinsics></fov>
              <data-output><bandwidth unit="Mbps">500</bandwidth>
                <bandwidth-compressed unit="Mbps">50</bandwidth-compressed>
                <frame-size unit="MB">6.2</frame-size></data-output>
              <camera-params><shutter>global</shutter><hdr>true</hdr>
                <compression>h265</compression><trigger>external</trigger></camera-params>
            </optical>
            <optical type="lidar">
              <lidar-params><scan-type>spinning</scan-type><channels>32</channels>
                <points-per-second>600000</points-per-second><scan-rate unit="Hz">10</scan-rate>
                <returns>dual</returns><wavelength unit="nm">905</wavelength>
                <horizontal-fov unit="deg">360</horizontal-fov>
                <vertical-fov unit="deg" min="-15" max="15"/></lidar-params>
            </optical>
          </sensor>
        </comp></hcdf>"#,
    );
    let cam = &doc.comp[0].sensor[0].optical[0];
    let intr = cam.fov[0].intrinsics.as_ref().expect("intrinsics");
    assert_eq!(intr.width.as_deref(), Some("1920"));
    assert_eq!(intr.cy.as_deref(), Some("540"));
    let co = cam.camera_params.as_ref().expect("camera-params");
    assert_eq!(co.shutter.as_deref(), Some("global"));
    assert_eq!(co.compression.as_deref(), Some("h265"));
    assert_eq!(
        cam.data_output
            .as_ref()
            .and_then(|d| d.bandwidth.as_ref())
            .and_then(|b| b.value.as_deref()),
        Some("500")
    );
    let lp = doc.comp[0].sensor[0].optical[1]
        .lidar_params
        .as_ref()
        .expect("lidar-params");
    assert_eq!(lp.channels.as_deref(), Some("32"));
    assert_eq!(lp.scan_type.as_deref(), Some("spinning"));
    assert_eq!(
        lp.scan_rate.as_ref().and_then(|v| v.value.as_deref()),
        Some("10")
    );
}

#[test]
fn rf_radar_gnss_params_roundtrip() {
    let doc = roundtrip(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <sensor name="radar">
            <rf type="radar">
              <radar-params><frequency unit="GHz">77</frequency><modulation>fmcw</modulation>
                <azimuth-fov unit="deg">120</azimuth-fov><elevation-fov unit="deg">30</elevation-fov>
                <range-resolution unit="m">0.04</range-resolution>
                <velocity-range unit="m/s" min="-50" max="50"/>
                <max-detections>256</max-detections><mimo>true</mimo></radar-params>
              <data-output><bandwidth unit="Mbps">12</bandwidth></data-output>
            </rf>
          </sensor>
          <sensor name="gnss">
            <rf type="gnss">
              <gnss-params><constellation>gps</constellation><constellation>galileo</constellation>
                <frequency>L1</frequency><frequency>L5</frequency><rtk>true</rtk>
                <accuracy-standalone unit="m">1.5</accuracy-standalone>
                <accuracy-rtk unit="cm">2</accuracy-rtk><pps>true</pps></gnss-params>
            </rf>
          </sensor>
        </comp></hcdf>"#,
    );
    let radar = doc.comp[0].sensor[0].rf[0]
        .radar_params
        .as_ref()
        .expect("radar-params");
    assert_eq!(radar.modulation.as_deref(), Some("fmcw"));
    assert_eq!(radar.max_detections.as_deref(), Some("256"));
    assert_eq!(
        radar.frequency.as_ref().and_then(|v| v.value.as_deref()),
        Some("77")
    );
    assert!(doc.comp[0].sensor[0].rf[0].data_output.is_some());
    let gnss = doc.comp[0].sensor[1].rf[0]
        .gnss_params
        .as_ref()
        .expect("gnss-params");
    assert_eq!(gnss.constellation, vec!["gps", "galileo"]);
    assert_eq!(gnss.frequency, vec!["L1", "L5"]);
    assert_eq!(gnss.pps.as_deref(), Some("true"));
}

#[test]
fn imu_fifo_roundtrips() {
    let doc = roundtrip(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <sensor name="imu"><inertial type="accel_gyro">
            <accel><range unit="g">16</range><fifo depth="512" watermark="64"/></accel>
            <gyro><range unit="dps">2000</range><fifo depth="256" watermark="32"/></gyro>
          </inertial></sensor>
        </comp></hcdf>"#,
    );
    let imu = &doc.comp[0].sensor[0].inertial[0];
    let af = imu
        .accel
        .as_ref()
        .and_then(|a| a.fifo.as_ref())
        .expect("accel fifo");
    assert_eq!(af.depth.as_deref(), Some("512"));
    assert_eq!(af.watermark.as_deref(), Some("64"));
    let gf = imu
        .gyro
        .as_ref()
        .and_then(|g| g.fifo.as_ref())
        .expect("gyro fifo");
    assert_eq!(gf.depth.as_deref(), Some("256"));
}

#[test]
fn battery_power_source_roundtrips() {
    let doc = roundtrip(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <power-source name="main"><battery>
            <chemistry>li-ion</chemistry><cells-series>6</cells-series><cells-parallel>2</cells-parallel>
            <nominal-voltage unit="V">22.2</nominal-voltage><capacity unit="Ah">10</capacity>
            <energy unit="Wh">222</energy><max-discharge unit="C">25</max-discharge>
            <min-voltage unit="V">18</min-voltage><max-voltage unit="V">25.2</max-voltage>
          </battery></power-source>
        </comp></hcdf>"#,
    );
    let bat = doc.comp[0].power_source[0]
        .battery
        .as_ref()
        .expect("battery");
    assert_eq!(bat.chemistry.as_deref(), Some("li-ion"));
    assert_eq!(bat.cells_series.as_deref(), Some("6"));
    assert_eq!(
        bat.energy.as_ref().and_then(|v| v.value.as_deref()),
        Some("222")
    );
    assert_eq!(
        bat.max_voltage.as_ref().and_then(|v| v.value.as_deref()),
        Some("25.2")
    );
}

#[test]
fn prop_surface_and_motor_thrust_roundtrip() {
    let doc = roundtrip(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <dynamic-surface name="rotor"><prop>
            <diameter unit="in">10</diameter><pitch unit="in">4.5</pitch>
            <blades>2</blades><direction>cw</direction></prop></dynamic-surface>
          <motor name="m1" type="bldc">
            <thrust-axis>+z</thrust-axis><max-thrust unit="N">15</max-thrust></motor>
        </comp></hcdf>"#,
    );
    let prop = doc.comp[0].dynamic_surface[0].prop.as_ref().expect("prop");
    assert_eq!(prop.blades.as_deref(), Some("2"));
    assert_eq!(prop.direction.as_deref(), Some("cw"));
    assert_eq!(
        prop.diameter.as_ref().and_then(|v| v.value.as_deref()),
        Some("10")
    );
    let motor = &doc.comp[0].motor[0];
    assert_eq!(motor.thrust_axis.as_deref(), Some("+z"));
    assert_eq!(
        motor.max_thrust.as_ref().and_then(|v| v.value.as_deref()),
        Some("15")
    );
}

#[test]
fn connectivity_ports_functions_and_antennas_roundtrip() {
    let doc = roundtrip(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <port name="uplink"><capabilities>
            <purpose value="communication"/><carrier value="electrical"/>
            <profile id="hcdf:rs-485"/><rate min="1000000" max="12000000" nominal="1000000" unit="bit/s"/>
          </capabilities><channel name="rx" role="hcdf:receive"/><channel name="tx" role="hcdf:transmit"/></port>
          <port name="downlink"/><port name="rf-feed"/><port name="rf-space"/>
          <antenna name="array">
            <conducted-port component="c" port="rf-feed"/>
            <radiated-port component="c" port="rf-space"/>
          </antenna>
          <switch name="fabric">
            <input><channel-ref component="c" port="uplink" channel="rx"/></input>
            <output><channel-ref component="c" port="uplink" channel="tx"/></output>
            <bidirectional><port-ref component="c" port="downlink"/></bidirectional>
          </switch>
        </comp></hcdf>"#,
    );
    let caps = doc.comp[0].port[0]
        .capabilities
        .as_ref()
        .expect("port capabilities");
    assert_eq!(caps.profile[0].id, "hcdf:rs-485");
    assert_eq!(
        caps.rate.as_ref().and_then(|rate| rate.nominal),
        Some(1_000_000.0)
    );
    assert_eq!(doc.comp[0].port[0].channel.len(), 2);
    let antenna = &doc.comp[0].antenna[0];
    assert_eq!(antenna.name, "array");
    assert_eq!(
        antenna
            .conducted_port
            .as_ref()
            .map(|port| port.port.as_str()),
        Some("rf-feed")
    );
    assert_eq!(antenna.radiated_port.port, "rf-space");
    let switch = &doc.comp[0].switch[0];
    assert_eq!(switch.name, "fabric");
    assert_eq!(
        (
            switch.input.len(),
            switch.output.len(),
            switch.bidirectional.len()
        ),
        (1, 1, 1)
    );
}

#[test]
fn bus_selection_and_chain_route_roundtrip() {
    let doc = roundtrip(
        r#"<hcdf name="x" version="1.0">
          <comp name="a"><port name="p"/></comp><comp name="b"><port name="p"/></comp>
          <bus name="control"><description>actuator bus</description>
            <selected purpose="communication" carrier="electrical"><profile id="hcdf:rs-485"/><rate><nominal value="1000000" unit="bit/s"/></rate></selected>
            <participant name="drive-a" role="hcdf:controller"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
            <participant name="drive-b" role="hcdf:node"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
          </bus>
          <chain name="routed"><selected purpose="communication" carrier="electrical"><profile id="hcdf:ethercat"/></selected>
            <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
            <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
            <hop name="h0" processing-delay-ns="25"><owner><component-ref component="a"/></owner></hop>
            <hop name="h1"><owner><component-ref component="b"/></owner></hop>
            <leg name="forward"><from><hop-ref network="routed" hop="h0"/><participant-ref network="routed" participant="a"/></from><to><hop-ref network="routed" hop="h1"/><participant-ref network="routed" participant="b"/></to></leg>
          </chain>
        </hcdf>"#,
    );
    let bus = &doc.bus[0];
    assert_eq!(bus.description.as_deref(), Some("actuator bus"));
    assert_eq!(bus.selected.as_ref().unwrap().profile[0].id, "hcdf:rs-485");
    assert_eq!(bus.participant.len(), 2);
    let chain = &doc.chain[0];
    assert_eq!(
        (chain.participant.len(), chain.hop.len(), chain.leg.len()),
        (2, 2, 1)
    );
    assert_eq!(chain.hop[0].processing_delay_ns, Some(25));
}

/// Like [`roundtrip`], and additionally assert the SERIALIZED output validates against the frozen
/// `hcdf.xsd` (feature `xsd`). A newly-typed sensor sub-tree must both survive the typed-model
/// round-trip AND remain schema-valid on the way out.
fn roundtrip_xsd(src: &str) -> Hcdf {
    let doc = Hcdf::from_xml_str(src).expect("parse");
    let out = doc.to_xml_string().expect("serialize");
    #[cfg(feature = "xsd")]
    {
        let issues = hcdformat::validate_xsd(&out);
        assert!(
            issues.is_empty(),
            "serialized output is not XSD-valid: {issues:?}\n{out}"
        );
    }
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "round-trip mismatch; serialized = {out}");
    doc
}

#[test]
fn fluid_barometer_roundtrips_all_fields() {
    let doc = roundtrip_xsd(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <sensor name="baro" update-rate="100">
            <fluid type="barometer">
              <driver name="bmp390"/>
              <resolution unit="Pa">1.5</resolution>
              <noise type="gaussian"><stddev>2.0</stddev></noise>
              <medium>air</medium>
              <pressure-range unit="hPa" min="260" max="1260"/>
              <accuracy unit="hPa">0.5</accuracy>
              <temperature-range unit="degC" min="-40" max="85"/>
              <temperature-coefficient unit="Pa/K">0.5</temperature-coefficient>
              <reference-pressure unit="hPa">1013.25</reference-pressure>
              <altitude-range unit="m" min="-500" max="9000"/>
              <altitude-resolution unit="m">0.1</altitude-resolution>
            </fluid>
          </sensor>
        </comp></hcdf>"#,
    );
    let f = &doc.comp[0].sensor[0].fluid[0];
    assert_eq!(f.type_.as_deref(), Some("barometer"));
    assert_eq!(
        f.driver.as_ref().and_then(|d| d.name.as_deref()),
        Some("bmp390")
    );
    assert_eq!(f.medium.as_deref(), Some("air"));
    let pr = f.pressure_range.as_ref().expect("pressure-range");
    assert_eq!(pr.min.as_deref(), Some("260"));
    assert_eq!(pr.max.as_deref(), Some("1260"));
    assert_eq!(pr.unit.as_deref(), Some("hPa"));
    assert_eq!(
        f.reference_pressure
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("1013.25")
    );
    assert_eq!(
        f.temperature_coefficient
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("0.5")
    );
    assert_eq!(
        f.altitude_resolution
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("0.1")
    );
    assert_eq!(
        f.accuracy.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.5")
    );
}

#[test]
fn fluid_airspeed_pitot_roundtrips_probe_and_outputs() {
    let doc = roundtrip_xsd(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <sensor name="pitot" update-rate="50">
            <fluid type="airspeed">
              <driver name="ms4525do"/>
              <resolution unit="Pa">0.1</resolution>
              <medium>air</medium>
              <pressure-range unit="Pa" min="0" max="6900"/>
              <accuracy unit="%FS">0.25</accuracy>
              <reference-density unit="kg/m3">1.225</reference-density>
              <airspeed-range unit="m/s" min="0" max="100"/>
              <airspeed-resolution unit="m/s">0.1</airspeed-resolution>
              <airspeed-output>IAS</airspeed-output>
              <airspeed-output>CAS</airspeed-output>
              <airspeed-output>TAS</airspeed-output>
              <probe type="pitot_static" static-port="true" total-port="true" ports="1"/>
            </fluid>
          </sensor>
        </comp></hcdf>"#,
    );
    let f = &doc.comp[0].sensor[0].fluid[0];
    assert_eq!(f.type_.as_deref(), Some("airspeed"));
    assert_eq!(
        f.reference_density
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("1.225")
    );
    assert_eq!(
        f.airspeed_range.as_ref().and_then(|v| v.max.as_deref()),
        Some("100")
    );
    assert_eq!(f.airspeed_output, vec!["IAS", "CAS", "TAS"]);
    let probe = f.probe.as_ref().expect("probe");
    assert_eq!(probe.type_.as_deref(), Some("pitot_static"));
    assert_eq!(probe.static_port.as_deref(), Some("true"));
    assert_eq!(probe.total_port.as_deref(), Some("true"));
    assert_eq!(probe.ports.as_deref(), Some("1"));
}

#[test]
fn lidar_range_and_scan_pattern_roundtrip() {
    let doc = roundtrip_xsd(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <sensor name="lidar">
            <optical type="lidar">
              <lidar-params>
                <channels>16</channels>
                <horizontal-fov unit="deg">360</horizontal-fov>
                <range><min unit="m">0.1</min><max unit="m">100</max><resolution unit="m">0.03</resolution></range>
                <scan-pattern>
                  <horizontal samples="1875" resolution="1" min-angle="-3.14159" max-angle="3.14159"/>
                  <vertical samples="16" resolution="1" min-angle="-0.261799" max-angle="0.261799"/>
                </scan-pattern>
              </lidar-params>
            </optical>
          </sensor>
        </comp></hcdf>"#,
    );
    let lp = doc.comp[0].sensor[0].optical[0]
        .lidar_params
        .as_ref()
        .expect("lidar-params");
    let rng = lp.range.as_ref().expect("range");
    assert_eq!(
        rng.min.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.1")
    );
    assert_eq!(
        rng.max.as_ref().and_then(|v| v.value.as_deref()),
        Some("100")
    );
    assert_eq!(
        rng.resolution.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.03")
    );
    let sp = lp.scan_pattern.as_ref().expect("scan-pattern");
    let h = sp.horizontal.as_ref().expect("horizontal");
    assert_eq!(h.samples.as_deref(), Some("1875"));
    assert_eq!(h.min_angle.as_deref(), Some("-3.14159"));
    assert_eq!(h.max_angle.as_deref(), Some("3.14159"));
    let v = sp.vertical.as_ref().expect("vertical");
    assert_eq!(v.samples.as_deref(), Some("16"));
    assert_eq!(v.resolution.as_deref(), Some("1"));
}

#[test]
fn camera_fov_skew_lens_distortion_calibration_roundtrip() {
    let doc = roundtrip_xsd(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <sensor name="fisheye">
            <optical type="camera">
              <fov name="main" color="0 1 0 0.3">
                <intrinsics><width>1280</width><height>1024</height><format>MONO8</format>
                  <fx>800</fx><fy>800</fy><cx>640</cx><cy>512</cy><s>0.5</s></intrinsics>
                <distortion><k1>-0.28</k1><k2>0.1</k2><k3>-0.02</k3><p1>0.001</p1><p2>-0.001</p2></distortion>
                <lens type="custom"><cutoff-angle unit="rad">1.5708</cutoff-angle>
                  <scale-to-hfov>true</scale-to-hfov>
                  <custom-function><c1>1.05</c1><c2>4</c2><c3>0</c3><f>1</f><fun>tan</fun></custom-function></lens>
                <calibration><fx>798.2</fx><fy>799.1</fy><cx>639.5</cx><cy>511.8</cy>
                  <k1>-0.279</k1><reprojection-error unit="px">0.21</reprojection-error>
                  <date>2026-06-01</date><temperature unit="degC">21</temperature>
                  <method>charuco-5x7-40mm</method><images>42</images></calibration>
                <noise type="gaussian"><stddev>0.5</stddev></noise>
              </fov>
            </optical>
          </sensor>
        </comp></hcdf>"#,
    );
    let fov = &doc.comp[0].sensor[0].optical[0].fov[0];
    let intr = fov.intrinsics.as_ref().expect("intrinsics");
    assert_eq!(intr.s.as_deref(), Some("0.5"));
    let dist = fov.distortion.as_ref().expect("distortion");
    assert_eq!(dist.k1.as_deref(), Some("-0.28"));
    assert_eq!(dist.p2.as_deref(), Some("-0.001"));
    let lens = fov.lens.as_ref().expect("lens");
    assert_eq!(lens.type_.as_deref(), Some("custom"));
    assert_eq!(
        lens.cutoff_angle.as_ref().and_then(|v| v.value.as_deref()),
        Some("1.5708")
    );
    assert_eq!(lens.scale_to_hfov.as_deref(), Some("true"));
    let cf = lens.custom_function.as_ref().expect("custom-function");
    assert_eq!(cf.c1.as_deref(), Some("1.05"));
    assert_eq!(cf.fun.as_deref(), Some("tan"));
    let cal = fov.calibration.as_ref().expect("calibration");
    assert_eq!(cal.fx.as_deref(), Some("798.2"));
    assert_eq!(
        cal.reprojection_error
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("0.21")
    );
    assert_eq!(cal.method.as_deref(), Some("charuco-5x7-40mm"));
    assert_eq!(cal.images.as_deref(), Some("42"));
    assert_eq!(
        fov.noise.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.5")
    );
}

#[test]
fn audio_extras_and_active_roundtrip() {
    let doc = roundtrip_xsd(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <sensor name="ultra">
            <audio type="ultrasonic" active="true">
              <driver name="hc-sr04"/>
              <range unit="m">4</range>
              <frequency-range unit="kHz" min="38" max="42"/>
              <sensitivity unit="dB">120</sensitivity>
              <channels>1</channels>
              <beam-width unit="deg">15</beam-width>
            </audio>
          </sensor>
        </comp></hcdf>"#,
    );
    let a = &doc.comp[0].sensor[0].audio[0];
    assert_eq!(a.type_.as_deref(), Some("ultrasonic"));
    assert_eq!(a.active.as_deref(), Some("true"));
    assert_eq!(
        a.frequency_range.as_ref().and_then(|v| v.max.as_deref()),
        Some("42")
    );
    assert_eq!(
        a.sensitivity.as_ref().and_then(|v| v.value.as_deref()),
        Some("120")
    );
    assert_eq!(a.channels.as_deref(), Some("1"));
    assert_eq!(
        a.beam_width.as_ref().and_then(|v| v.value.as_deref()),
        Some("15")
    );
}

#[test]
fn tactile_fixture_extras_are_typed_and_roundtrip() {
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/valid/sensor-tactile.hcdf"
    ));
    let doc = roundtrip_xsd(src);
    // fingertip capacitive array: all six tactile extras present and now typed
    let tip = &doc.comp[0].sensor[0].tactile[0];
    assert_eq!(tip.type_.as_deref(), Some("capacitive"));
    assert_eq!(tip.rows.as_deref(), Some("4"));
    assert_eq!(tip.cols.as_deref(), Some("4"));
    assert_eq!(tip.taxels.as_deref(), Some("16"));
    assert_eq!(
        tip.spatial_resolution
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("2.5")
    );
    let pr = tip.pressure_range.as_ref().expect("pressure-range");
    assert_eq!(pr.min.as_deref(), Some("0.1"));
    assert_eq!(pr.max.as_deref(), Some("100"));
    assert_eq!(
        tip.sensing_area.as_ref().and_then(|v| v.value.as_deref()),
        Some("100")
    );
    // optical gel sensor uses the inherited range/resolution plus sensing-area
    let gel = &doc.comp[0].sensor[1].tactile[1];
    assert_eq!(gel.type_.as_deref(), Some("optical"));
    assert_eq!(
        gel.range.as_ref().and_then(|v| v.value.as_deref()),
        Some("5")
    );
    assert_eq!(
        gel.sensing_area.as_ref().and_then(|v| v.value.as_deref()),
        Some("314")
    );
}

/// Like [`roundtrip_xsd`], and additionally assert the serialization is a byte-identical FIXPOINT: the
/// second serialize equals the first, so no authored value is silently reformatted or dropped across
/// parse -> serialize -> parse -> serialize.
fn roundtrip_xsd_bytes(src: &str) -> Hcdf {
    let doc = Hcdf::from_xml_str(src).expect("parse");
    let out = doc.to_xml_string().expect("serialize");
    #[cfg(feature = "xsd")]
    {
        let issues = hcdformat::validate_xsd(&out);
        assert!(
            issues.is_empty(),
            "serialized output is not XSD-valid: {issues:?}\n{out}"
        );
    }
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "round-trip mismatch; serialized = {out}");
    let out2 = doc2.to_xml_string().expect("re-serialize");
    assert_eq!(out, out2, "serialization is not a byte-identical fixpoint");
    doc
}

/// Round-trip fidelity: the motor thermal (thermal-resistance/max-temperature), BLDC
/// (pole-pairs) and ICE/linear (displacement/cylinders/stroke) leaves, clean port capabilities, and
/// the audio sensor extras (all typed and preserved on HCDF-to-HCDF conversion)
/// survive a byte-identical round-trip.
#[test]
fn motor_detail_port_capabilities_and_audio_roundtrip_byte_identical() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <port name="eth0"><capabilities>
            <purpose value="communication"/><carrier value="electrical"/>
            <profile id="hcdf:10base-t1s"/><rate nominal="10000000" unit="bit/s"/>
          </capabilities></port>
          <sensor name="mic">
            <audio type="microphone" active="false">
              <frequency-range unit="Hz" min="20" max="20000"/>
              <sensitivity unit="dBV/Pa">-38</sensitivity>
              <channels>2</channels>
              <beam-width unit="deg">60</beam-width>
            </audio>
          </sensor>
          <motor name="m1" type="bldc">
            <resistance unit="ohm">0.5</resistance>
            <thermal-resistance unit="degC/W">1.8</thermal-resistance>
            <max-temperature unit="degC">120</max-temperature>
            <pole-pairs>15</pole-pairs>
            <displacement unit="cc">50</displacement>
            <cylinders>1</cylinders>
            <stroke unit="mm">120</stroke>
          </motor>
        </comp></hcdf>"#,
    );
    let caps = doc.comp[0].port[0]
        .capabilities
        .as_ref()
        .expect("capabilities");
    assert_eq!(caps.profile[0].id, "hcdf:10base-t1s");
    assert_eq!(
        caps.rate.as_ref().and_then(|rate| rate.nominal),
        Some(10_000_000.0)
    );
    let m = &doc.comp[0].motor[0];
    assert_eq!(
        m.thermal_resistance
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("1.8"),
        "thermal-resistance typed"
    );
    assert_eq!(
        m.max_temperature.as_ref().and_then(|v| v.value.as_deref()),
        Some("120")
    );
    assert_eq!(m.pole_pairs.as_deref(), Some("15"), "pole-pairs typed");
    assert_eq!(
        m.displacement.as_ref().and_then(|v| v.value.as_deref()),
        Some("50")
    );
    assert_eq!(m.cylinders.as_deref(), Some("1"), "cylinders typed");
    assert_eq!(
        m.stroke.as_ref().and_then(|v| v.value.as_deref()),
        Some("120")
    );
}

/// A radiated-RF star names its selected profile and channel, identifies its coordinator by a scoped
/// participant reference, and keeps every participant endpoint structurally explicit.
#[test]
fn network_star_infrastructure_topology_roundtrip_byte_identical() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0">
          <comp name="ap"><port name="rf"/></comp>
          <comp name="cam1"><port name="rf"/></comp>
          <comp name="cam2"><port name="rf"/></comp>
          <star name="fleet">
            <selected purpose="communication" carrier="radiated-rf"><profile id="ieee:802.11ax"/><rf><channel><numbered number="36"><center-frequency><nominal value="5.18" unit="GHz"/></center-frequency><bandwidth><nominal value="80" unit="MHz"/></bandwidth></numbered></channel></rf></selected>
            <coordinator><participant-ref network="fleet" participant="ap"/></coordinator>
            <participant name="ap" role="hcdf:coordinator"><endpoint><port-ref component="ap" port="rf"/></endpoint></participant>
            <participant name="cam1"><endpoint><port-ref component="cam1" port="rf"/></endpoint></participant>
            <participant name="cam2"><endpoint><port-ref component="cam2" port="rf"/></endpoint></participant>
          </star>
        </hcdf>"#,
    );
    let star = &doc.star[0];
    assert_eq!(star.name, "fleet");
    assert_eq!(
        star.selected.as_ref().unwrap().profile[0].id,
        "ieee:802.11ax"
    );
    assert!(star.selected.as_ref().unwrap().rf.is_some());
    assert_eq!(star.coordinator.participant.participant, "ap");
    assert_eq!(star.participant.len(), 3);
}

/// A link serializes its selection, configuration, and participants in schema order. Configuration
/// owns gPTP rather than a retired generic network wrapper.
#[test]
fn network_topology_and_config_children_serialize_in_xsd_sequence() {
    let doc = roundtrip_xsd(
        r#"<hcdf name="x" version="1.0">
          <comp name="a"><port name="eth0"/></comp><comp name="b"><port name="eth0"/></comp>
          <link name="uplink">
            <selected purpose="communication" carrier="electrical"><profile id="ieee:1000base-t"/></selected>
            <configuration><gptp-domain name="time" number="0"><clock name="a-clock" kind="ordinary" gm-capable="true"><participant-ref network="uplink" participant="a"/></clock></gptp-domain></configuration>
            <participant name="a"><endpoint><port-ref component="a" port="eth0"/></endpoint></participant>
            <participant name="b"><endpoint><port-ref component="b" port="eth0"/></endpoint></participant>
          </link>
        </hcdf>"#,
    );
    let link = &doc.link[0];
    assert_eq!(link.name, "uplink");
    let gptp = &link.configuration.as_ref().unwrap().gptp_domain[0];
    assert_eq!((gptp.name.as_str(), gptp.number), ("time", 0));
    let out = doc.to_xml_string().expect("serialize");
    let selected_at = out.find("<selected").expect("selection serialized");
    let configuration_at = out
        .find("<configuration")
        .expect("configuration serialized");
    let participant_at = out
        .find("<participant name")
        .expect("participant serialized");
    assert!(
        selected_at < configuration_at && configuration_at < participant_at,
        "link children must serialize in schema order: {out}"
    );
}

/// A point-to-point RF link uses ordinary functional participants at radiated ports. Antennas remain
/// physical component structures and the selected network carries profile and spectrum assignment.
#[test]
fn wireless_p2p_link_two_antennas_roundtrip_byte_identical() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0">
          <comp name="gcs"><port name="feed"/><port name="air"/><antenna name="radio"><conducted-port component="gcs" port="feed"/><radiated-port component="gcs" port="air"/></antenna></comp>
          <comp name="uav"><port name="feed"/><port name="air"/><antenna name="radio"><conducted-port component="uav" port="feed"/><radiated-port component="uav" port="air"/></antenna></comp>
          <link name="telemetry">
            <selected purpose="communication" carrier="radiated-rf"><profile id="ieee:802.11ax"/><rf><channel><frequency-defined><center-frequency><nominal value="60" unit="GHz"/></center-frequency><bandwidth><range min="1" max="2" nominal="2" unit="GHz"/></bandwidth></frequency-defined></channel></rf></selected>
            <participant name="gcs"><endpoint><port-ref component="gcs" port="air"/></endpoint></participant>
            <participant name="uav"><endpoint><port-ref component="uav" port="air"/></endpoint></participant>
          </link>
        </hcdf>"#,
    );
    let link = &doc.link[0];
    assert_eq!(link.name, "telemetry");
    assert_eq!(link.participant[0].name, "gcs");
    assert_eq!(link.participant[1].name, "uav");
    assert_eq!(
        link.selected.as_ref().unwrap().profile[0].id,
        "ieee:802.11ax"
    );
    assert!(link.selected.as_ref().unwrap().rf.is_some());
    assert_eq!(doc.comp[0].antenna[0].radiated_port.port, "air");
}

/// One bus exercises the full topology-owned configuration sequence: gPTP, traffic classes, gate
/// schedules, schedule assignments, PLCA, MACsec, and EEE. References remain scoped to named network
/// participants and policies instead of being encoded as combined strings.
#[test]
fn network_configuration_roundtrips_byte_identical() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0">
          <comp name="controller"><port name="eth"/></comp><comp name="node"><port name="eth"/></comp>
          <bus name="t1s"><description>10BASE-T1S multidrop</description>
            <selected purpose="communication" carrier="electrical"><profile id="hcdf:10base-t1s"/><rate><nominal value="10000000" unit="bit/s"/></rate></selected>
            <configuration>
              <gptp-domain name="time" number="0">
                <clock name="grandmaster" kind="ordinary" gm-capable="true" priority1="128" priority2="128" clock-class="6" clock-accuracy="32"><participant-ref network="t1s" participant="controller"/></clock>
                <port-defaults log-sync-interval="-3" log-announce-interval="0" log-pdelay-req-interval="0" announce-receipt-timeout="3" neighbor-prop-delay-threshold-ns="800"/>
              </gptp-domain>
              <traffic-class name="control" number="7" preemption="express"><description>hard real-time control</description><pcp value="7"/></traffic-class>
              <gate-schedule name="control-cycle" cycle-time-ns="1000000"><gate duration-ns="100000"><open><traffic-class-ref network="t1s" traffic-class="control"/></open></gate></gate-schedule>
              <schedule-assignment name="controller-schedule"><schedule-ref network="t1s" schedule="control-cycle"/><target><participant-ref network="t1s" participant="controller"/></target></schedule-assignment>
              <plca max-node-id="8" to-timer-bit-times="32">
                <node id="0" burst-count="0" burst-timer-bit-times="0"><participant-ref network="t1s" participant="controller"/></node>
                <node id="1" burst-count="0" burst-timer-bit-times="0"><participant-ref network="t1s" participant="node"/></node>
              </plca>
              <macsec><policy name="secure" enforcement="must-secure" cipher="ieee:gcm-aes-128" key-agreement="ieee:mka" confidentiality-offset="0" rekey-interval-ns="1000000000" credential-store-ref="keys/main"/><default-policy><macsec-policy-ref network="t1s" policy="secure"/></default-policy><override><target><network-ref network="t1s"/></target><macsec-policy-ref network="t1s" policy="secure"/></override></macsec>
              <eee default-mode="enabled"><override mode="disabled"><participant-ref network="t1s" participant="controller"/></override></eee>
            </configuration>
            <participant name="controller" role="hcdf:controller"><endpoint><port-ref component="controller" port="eth"/></endpoint></participant>
            <participant name="node" role="hcdf:node"><endpoint><port-ref component="node" port="eth"/></endpoint></participant>
          </bus>
        </hcdf>"#,
    );
    let bus = &doc.bus[0];
    assert_eq!(bus.description.as_deref(), Some("10BASE-T1S multidrop"));
    assert_eq!(
        bus.selected.as_ref().unwrap().profile[0].id,
        "hcdf:10base-t1s"
    );
    let config = bus.configuration.as_ref().expect("configuration");
    let gptp = &config.gptp_domain[0];
    assert_eq!(gptp.clock[0].clock_class, Some(6));
    assert_eq!(gptp.clock[0].clock_accuracy, Some(32));
    let defaults = gptp.port_defaults.as_ref().expect("port defaults");
    assert_eq!(defaults.neighbor_prop_delay_threshold_ns, Some(800));
    assert_eq!(
        config.traffic_class[0].description.as_deref(),
        Some("hard real-time control")
    );
    assert_eq!(config.gate_schedule[0].cycle_time_ns, 1_000_000);
    assert_eq!(config.schedule_assignment[0].target.len(), 1);
    let plca = config.plca.as_ref().expect("PLCA");
    assert_eq!((plca.max_node_id, plca.node.len()), (8, 2));
    let macsec = config.macsec.as_ref().expect("MACsec");
    assert_eq!(macsec.policy[0].cipher.as_deref(), Some("ieee:gcm-aes-128"));
    assert!(macsec.default_policy.is_some() && macsec.override_.len() == 1);
    let eee = config.eee.as_ref().expect("EEE");
    assert_eq!(eee.override_.len(), 1);
}

// ── transmission: multi-endpoint couplings (repeatable <motor>/<joint> + @role, HCDF 1.0) ──────────

/// A classic SIMPLE reduction (one `<motor>` + one `<joint>` endpoint, the pre-widening shape) must
/// still parse, validate against the frozen `hcdf.xsd`, and survive a byte-identical fixpoint after the
/// endpoints became repeatable `Vec`s.
#[test]
fn transmission_simple_single_endpoint_roundtrips_byte_identical() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0">
          <comp name="hip"><motor name="drive"/></comp>
          <transmission name="hip_trans" type="simple"><motor ref="hip/drive"/><joint ref="hip_joint"/><reduction>100</reduction></transmission>
        </hcdf>"#,
    );
    let tr = &doc.transmission[0];
    assert_eq!(tr.motor.len(), 1, "one motor endpoint");
    assert_eq!(tr.joint.len(), 1, "one joint endpoint");
    assert_eq!(tr.motor[0].ref_.as_deref(), Some("hip/drive"));
    assert_eq!(tr.joint[0].ref_.as_deref(), Some("hip_joint"));
    // A simple reduction leaves @role unset: its endpoints are unambiguous.
    assert!(
        tr.motor[0].role.is_none() && tr.joint[0].role.is_none(),
        "no @role on a simple reduction"
    );
}

/// A GEARBOX coupling: two `<joint>` endpoints geared by `<reduction>`, disambiguated by
/// `@role="reference"` / `@role="driven"`. Validates against the frozen `hcdf.xsd` (the widened
/// repeatable endpoints + the new `RoleType`) and round-trips byte-identically.
#[test]
fn transmission_gearbox_two_joint_roles_roundtrips_byte_identical() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0">
          <transmission name="knee_gearbox" type="gear"><joint ref="drive_joint" role="reference"/><joint ref="output_joint" role="driven"/><reduction>2.5</reduction></transmission>
        </hcdf>"#,
    );
    let tr = &doc.transmission[0];
    assert!(
        tr.motor.is_empty(),
        "a gearbox couples two joints, no motor endpoint"
    );
    assert_eq!(tr.joint.len(), 2, "two joint endpoints");
    assert_eq!(tr.joint[0].ref_.as_deref(), Some("drive_joint"));
    assert_eq!(tr.joint[0].role.as_deref(), Some("reference"));
    assert_eq!(tr.joint[1].ref_.as_deref(), Some("output_joint"));
    assert_eq!(tr.joint[1].role.as_deref(), Some("driven"));
    assert_eq!(
        tr.reduction.as_deref(),
        Some("2.5"),
        "the gear ratio between the two joints"
    );
}

/// A DIFFERENTIAL coupling: two `<motor>` inputs driving two `<joint>` outputs (`@role="input"` /
/// `@role="output"`): the shape the schema's own `differential` `TransmissionType` doc always implied
/// but had no endpoints for. Must validate against the frozen `hcdf.xsd` and round-trip byte-identically.
#[test]
fn transmission_differential_two_motor_two_joint_validates() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0">
          <comp name="wrist"><motor name="m1"/><motor name="m2"/></comp>
          <transmission name="wrist_diff" type="differential"><motor ref="wrist/m1" role="input"/><motor ref="wrist/m2" role="input"/><joint ref="pitch" role="output"/><joint ref="roll" role="output"/><reduction>1</reduction></transmission>
        </hcdf>"#,
    );
    let tr = &doc.transmission[0];
    assert_eq!(tr.motor.len(), 2, "two motor inputs");
    assert_eq!(tr.joint.len(), 2, "two joint outputs");
    assert!(
        tr.motor.iter().all(|e| e.role.as_deref() == Some("input")),
        "both motors are inputs"
    );
    assert!(
        tr.joint.iter().all(|e| e.role.as_deref() == Some("output")),
        "both joints are outputs"
    );
    assert_eq!(tr.motor[0].ref_.as_deref(), Some("wrist/m1"));
    assert_eq!(tr.joint[1].ref_.as_deref(), Some("roll"));
}

/// Port power capabilities use one quantity-range shape with explicit min, max, nominal, and unit
/// attributes. Optional bounds remain absent rather than being fabricated during serialization.
#[test]
fn port_power_capabilities_min_max_roundtrip_byte_identical() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <port name="pwr"><capabilities>
            <purpose value="power-delivery"/><carrier value="electrical"/>
            <voltage unit="V" min="9" max="36" nominal="12"/>
            <current unit="A" max="5" nominal="3"/>
            <power unit="W" max="180" nominal="100"/>
          </capabilities></port>
          <port name="pwr2"><capabilities>
            <current unit="A" nominal="2"/>
          </capabilities></port>
        </comp></hcdf>"#,
    );
    let caps = doc.comp[0].port[0]
        .capabilities
        .as_ref()
        .expect("port capabilities");
    let v = caps.voltage.as_ref().expect("voltage capability");
    assert_eq!(v.unit, "V");
    assert_eq!(v.minimum, Some(9.0), "voltage @min must not be dropped");
    assert_eq!(v.maximum, Some(36.0), "voltage @max must not be dropped");
    assert_eq!(v.nominal, Some(12.0), "voltage @nominal preserved");
    let i = caps.current.as_ref().expect("current capability");
    assert_eq!(i.unit, "A");
    assert_eq!(i.maximum, Some(5.0), "current @max must not be dropped");
    assert_eq!(i.nominal, Some(3.0));
    let p = caps.power.as_ref().expect("power capability");
    assert_eq!(p.maximum, Some(180.0), "power @max must not be dropped");
    assert_eq!(p.nominal, Some(100.0));
    // a bare current (no @min/@max) still parses + round-trips
    let bare = doc.comp[0].port[1]
        .capabilities
        .as_ref()
        .and_then(|c| c.current.as_ref())
        .unwrap();
    assert!(bare.maximum.is_none() && bare.nominal == Some(2.0));
}

/// Read a first-party corpus file from `<repo>/tests/valid` (the crate lives at `<repo>/rust/hcdformat-rs`).
fn read_valid(name: &str) -> Option<String> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/valid")
        .join(name);
    std::fs::read_to_string(p).ok()
}

/// `<power-source>` alt energy kinds. The model typed only `<battery>`, silently dropping
/// `<solar>`/`<supercapacitor>`/`<tank>`/`<fuel-cell>` (hcdf.xsd:1656/1686/1611/1632). Confirm with the
/// real `power-solar-supercap.hcdf` corpus doc (solar + supercapacitor) that these now parse + round-trip.
#[test]
fn power_source_solar_supercap_corpus_roundtrips() {
    let Some(xml) = read_valid("power-solar-supercap.hcdf") else {
        eprintln!("skipping: corpus not reachable");
        return;
    };
    let doc = Hcdf::from_xml_str(&xml).expect("parse corpus");
    let out = doc.to_xml_string().expect("serialize");
    #[cfg(feature = "xsd")]
    {
        let issues = hcdformat::validate_xsd(&out);
        assert!(
            issues.is_empty(),
            "serialized output is not XSD-valid: {issues:?}\n{out}"
        );
    }
    assert_eq!(
        doc,
        Hcdf::from_xml_str(&out).expect("re-parse"),
        "corpus did not round-trip"
    );
    let sources = &doc.comp[0].power_source;
    let solar = sources
        .iter()
        .find_map(|s| s.solar.as_ref())
        .expect("solar source not dropped");
    assert_eq!(solar.cell_type.as_deref(), Some("monocrystalline"));
    assert_eq!(
        solar.peak_power.as_ref().and_then(|v| v.value.as_deref()),
        Some("200")
    );
    assert_eq!(solar.efficiency.as_deref(), Some("0.22"));
    assert_eq!(solar.mppt.as_deref(), Some("true"));
    assert_eq!(
        solar
            .open_circuit_voltage
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("42")
    );
    let sc = sources
        .iter()
        .find_map(|s| s.supercapacitor.as_ref())
        .expect("supercapacitor not dropped");
    assert_eq!(
        sc.capacitance.as_ref().and_then(|v| v.value.as_deref()),
        Some("500")
    );
    assert_eq!(
        sc.esr.as_ref().and_then(|v| v.value.as_deref()),
        Some("3.2")
    );
    assert_eq!(
        sc.peak_current.as_ref().and_then(|v| v.value.as_deref()),
        Some("400")
    );
    assert_eq!(sc.cells_series.as_deref(), Some("18"));
    assert_eq!(sc.cycle_life.as_deref(), Some("1000000"));
}

/// `<tank>` (with nested `<flow>`) and `<fuel-cell>`, not in the corpus, exercised
/// synthetically. Must validate against the frozen `hcdf.xsd` and round-trip byte-identically.
#[test]
fn power_source_tank_fuelcell_roundtrips_byte_identical() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0"><comp name="c">
          <power-source name="h2-tank" role="primary"><tank>
            <fuel>hydrogen</fuel><volume unit="L">6.8</volume><pressure unit="bar">350</pressure>
            <energy unit="Wh">2200</energy>
            <flow><method>regulated</method>
              <outlet-pressure unit="bar" min="1.5" max="3.0">2.0</outlet-pressure>
              <max-flow-rate unit="SLPM">30</max-flow-rate><peak-flow-rate unit="SLPM">45</peak-flow-rate>
              <regulator>solenoid-proportional</regulator></flow>
          </tank></power-source>
          <power-source name="pem" role="primary"><fuel-cell>
            <type>pem</type><fuel>hydrogen</fuel><rated-power unit="W">250</rated-power>
            <peak-power unit="W">400</peak-power><efficiency>0.55</efficiency>
            <output-voltage unit="V" min="18" max="30">24</output-voltage>
          </fuel-cell></power-source>
        </comp></hcdf>"#,
    );
    let tank = doc.comp[0].power_source[0]
        .tank
        .as_ref()
        .expect("tank not dropped");
    assert_eq!(tank.fuel.as_deref(), Some("hydrogen"));
    assert_eq!(
        tank.pressure.as_ref().and_then(|v| v.value.as_deref()),
        Some("350")
    );
    let flow = tank.flow.as_ref().expect("flow not dropped");
    assert_eq!(flow.method.as_deref(), Some("regulated"));
    assert_eq!(
        flow.outlet_pressure.as_ref().and_then(|v| v.max.as_deref()),
        Some("3.0")
    );
    assert_eq!(
        flow.peak_flow_rate
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("45")
    );
    let fc = doc.comp[0].power_source[1]
        .fuel_cell
        .as_ref()
        .expect("fuel-cell not dropped");
    assert_eq!(fc.type_.as_deref(), Some("pem"));
    assert_eq!(fc.efficiency.as_deref(), Some("0.55"));
    assert_eq!(
        fc.output_voltage.as_ref().and_then(|v| v.min.as_deref()),
        Some("18")
    );
}

/// `<transmission>` dropped the entire `<spring>` subtree (SEA/VSA compliance). Add
/// `spring: Vec<TransmissionSpring>` (hcdf.xsd:2928, type transmission_spring:2865). Confirm with the
/// real transmission-spring.hcdf corpus (SEA, PEA, CPEA, AE-PEA, SEA+PEA) that springs + their
/// @placement/@type/@clutch/@equilibrium-actuator attributes and leaves round-trip losslessly.
#[test]
fn transmission_spring_corpus_roundtrips() {
    let Some(xml) = read_valid("transmission-spring.hcdf") else {
        eprintln!("skipping: corpus not reachable");
        return;
    };
    let doc = Hcdf::from_xml_str(&xml).expect("parse corpus");
    let out = doc.to_xml_string().expect("serialize");
    #[cfg(feature = "xsd")]
    {
        let issues = hcdformat::validate_xsd(&out);
        assert!(
            issues.is_empty(),
            "serialized output is not XSD-valid: {issues:?}\n{out}"
        );
    }
    assert_eq!(
        doc,
        Hcdf::from_xml_str(&out).expect("re-parse"),
        "corpus did not round-trip"
    );
    let by_name = |n: &str| {
        doc.transmission
            .iter()
            .find(|t| t.name.as_deref() == Some(n))
            .unwrap()
    };
    // SEA: one series spring with torque-sensing
    let sea = &by_name("shoulder_sea").spring;
    assert_eq!(sea.len(), 1);
    assert_eq!(sea[0].placement.as_deref(), Some("series"));
    assert_eq!(sea[0].torque_sensing.as_deref(), Some("spring-deflection"));
    assert_eq!(
        sea[0].stiffness.as_ref().and_then(|v| v.value.as_deref()),
        Some("4.5")
    );
    assert_eq!(
        sea[0]
            .max_deflection
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("0.15")
    );
    // CPEA: parallel spring + clutch + clutch-power/engage-time
    let cpea = &by_name("knee_cpea").spring[0];
    assert_eq!(cpea.clutch.as_deref(), Some("electromagnetic"));
    assert_eq!(
        cpea.clutch_power.as_ref().and_then(|v| v.value.as_deref()),
        Some("2.5")
    );
    assert_eq!(
        cpea.engage_time.as_ref().and_then(|v| v.value.as_deref()),
        Some("5")
    );
    // AE-PEA: equilibrium-actuator attr + equilibrium-range with min/max
    let aepea = &by_name("hip_aepea").spring[0];
    assert_eq!(aepea.equilibrium_actuator.as_deref(), Some("arm.eq_motor"));
    let er = aepea.equilibrium_range.as_ref().expect("equilibrium-range");
    assert_eq!(
        (er.min.as_deref(), er.max.as_deref()),
        (Some("-1.57"), Some("0.0"))
    );
    // SEA+PEA combo: two springs
    assert_eq!(
        by_name("ankle_combo").spring.len(),
        2,
        "both springs preserved"
    );
}

/// `<dynamic-surface>` typed only prop+wheel, dropping the `<gripper>` end-effector
/// subtree (hcdf.xsd:1529, type gripper_surface:1477). Confirm with the real surface-gripper.hcdf
/// corpus (mechanical/suction/magnetic/adhesive) that gripper surfaces round-trip losslessly.
#[test]
fn dynamic_surface_gripper_corpus_roundtrips() {
    let Some(xml) = read_valid("surface-gripper.hcdf") else {
        eprintln!("skipping: corpus not reachable");
        return;
    };
    let doc = Hcdf::from_xml_str(&xml).expect("parse corpus");
    let out = doc.to_xml_string().expect("serialize");
    #[cfg(feature = "xsd")]
    {
        let issues = hcdformat::validate_xsd(&out);
        assert!(
            issues.is_empty(),
            "serialized output is not XSD-valid: {issues:?}\n{out}"
        );
    }
    assert_eq!(
        doc,
        Hcdf::from_xml_str(&out).expect("re-parse"),
        "corpus did not round-trip"
    );
    let grip = |c: usize| {
        doc.comp[c].dynamic_surface[0]
            .gripper
            .as_ref()
            .expect("gripper not dropped")
    };
    let mech = grip(0);
    assert_eq!(mech.type_.as_deref(), Some("mechanical"));
    assert_eq!(
        mech.grip_force.as_ref().and_then(|v| v.value.as_deref()),
        Some("40")
    );
    assert_eq!(mech.shore_hardness.as_deref(), Some("A30"));
    let fr = mech.friction.as_ref().expect("gripper friction");
    assert_eq!(
        (fr.static_.as_deref(), fr.dynamic.as_deref()),
        (Some("0.9"), Some("0.7"))
    );
    assert_eq!(
        grip(1)
            .vacuum_level
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("-65")
    );
    assert_eq!(
        grip(2)
            .magnetic_force
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("500")
    );
    assert_eq!(grip(3).type_.as_deref(), Some("adhesive"));
}

/// `<dynamic-surface>` also dropped the aero/marine/tracked kinds: aerofoil
/// (hcdf.xsd:1381), hydrofoil (1405), control-surface (1423), track (1459). Not in the corpus, so
/// exercised synthetically across four comps: each must validate against the frozen hcdf.xsd and
/// round-trip byte-identically (declared in XSD sequence order prop..aerofoil..control-surface..track).
#[test]
fn dynamic_surface_aero_marine_tracked_roundtrips_byte_identical() {
    let doc = roundtrip_xsd_bytes(
        r#"<hcdf name="x" version="1.0">
          <comp name="wing"><dynamic-surface name="main_wing"><aerofoil>
            <span unit="m">1.2</span><chord unit="m">0.25</chord><profile>NACA2412</profile>
            <sweep unit="deg">5</sweep><dihedral unit="deg">3</dihedral><area unit="m2">0.3</area>
          </aerofoil></dynamic-surface></comp>
          <comp name="foil"><dynamic-surface name="dive_plane"><hydrofoil>
            <span unit="m">0.4</span><chord unit="m">0.1</chord><profile>NACA0012</profile>
            <area unit="m2">0.04</area>
          </hydrofoil></dynamic-surface></comp>
          <comp name="ail"><dynamic-surface name="aileron_l"><control-surface type="aileron">
            <chord-ratio>0.25</chord-ratio><span-fraction>0.4</span-fraction>
            <deflection unit="deg" min="-25" max="25"/>
          </control-surface></dynamic-surface></comp>
          <comp name="tread"><dynamic-surface name="left_track"><track>
            <width unit="m">0.3</width><length unit="m">0.9</length>
            <ground-pressure unit="kPa">25</ground-pressure><friction static="1.2" dynamic="1.0"/>
          </track></dynamic-surface></comp>
        </hcdf>"#,
    );
    let aero = doc.comp[0].dynamic_surface[0]
        .aerofoil
        .as_ref()
        .expect("aerofoil not dropped");
    assert_eq!(aero.profile.as_deref(), Some("NACA2412"));
    assert_eq!(
        aero.dihedral.as_ref().and_then(|v| v.value.as_deref()),
        Some("3")
    );
    let hydro = doc.comp[1].dynamic_surface[0]
        .hydrofoil
        .as_ref()
        .expect("hydrofoil not dropped");
    assert_eq!(
        hydro.area.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.04")
    );
    let cs = doc.comp[2].dynamic_surface[0]
        .control_surface
        .as_ref()
        .expect("control-surface not dropped");
    assert_eq!(cs.type_.as_deref(), Some("aileron"));
    assert_eq!(cs.chord_ratio.as_deref(), Some("0.25"));
    let defl = cs.deflection.as_ref().expect("deflection");
    assert_eq!(
        (defl.min.as_deref(), defl.max.as_deref()),
        (Some("-25"), Some("25"))
    );
    let track = doc.comp[3].dynamic_surface[0]
        .track
        .as_ref()
        .expect("track not dropped");
    assert_eq!(
        track
            .ground_pressure
            .as_ref()
            .and_then(|v| v.value.as_deref()),
        Some("25")
    );
    assert_eq!(
        track.friction.as_ref().and_then(|f| f.static_.as_deref()),
        Some("1.2")
    );
}
