# HCDF Extension Example: IMU Stability Parameters

## Why This Is an Extension (Not Core)

Most HCDF users describe consumer/automotive MEMS IMUs (ICM45686, BMI088, BMI270)
for mobile robots. For these sensors, the core HCDF sensor model is sufficient:
- Noise density (stddev in noise model) — what the filter actually uses
- ODR, bandwidth, FIFO — control loop timing
- Range, resolution — dynamic range
- Driver + axis-align — Zephyr integration

**Allan variance parameters** (bias stability, scale factor error, g-sensitivity)
matter primarily for:
- Tactical/navigation-grade IMUs ($1000+: ADIS16465, KVH1750, HG1700)
- Aerospace and defense applications
- Underwater navigation (GPS-denied for long periods)
- INS/GNSS integration where dead reckoning matters over minutes/hours

For a mobile robot that re-observes landmarks every few seconds, the navigation
filter re-estimates bias faster than Allan variance specs become relevant.
The core HCDF shouldn't carry fields that 99% of users don't need.

## How to Use the Extension

The `<extension>` element on a `<comp>` allows vendor-specific or domain-specific
data using a reverse-DNS domain identifier. An aerospace team can add IMU stability
data without modifying the HCDF core schema:

```xml
<comp name="ins-nav-unit" role="sensor">
  <description>Tactical-grade INS for GPS-denied navigation</description>

  <sensor name="tactical_imu" update-rate="400">
    <inertial type="accel_gyro">
      <driver name="adis16465-2"/>

      <accel>
        <range unit="g">10</range>
        <resolution unit="ug">0.25</resolution>
        <odr unit="Hz">2000</odr>
        <bandwidth unit="Hz">500</bandwidth>
        <fifo depth="512" watermark="5"/>
        <noise type="gaussian">
          <stddev>0.000023</stddev>
          <bias-mean>0.004</bias-mean>
          <bias-stddev>0.001</bias-stddev>
        </noise>
      </accel>

      <gyro>
        <range unit="dps">500</range>
        <resolution unit="dps">0.00625</resolution>
        <odr unit="Hz">2000</odr>
        <bandwidth unit="Hz">500</bandwidth>
        <fifo depth="512" watermark="5"/>
        <noise type="gaussian">
          <stddev>0.0035</stddev>
          <bias-mean>0.02</bias-mean>
          <bias-stddev>0.005</bias-stddev>
        </noise>
      </gyro>
    </inertial>
  </sensor>

  <!-- IMU stability extension: Allan variance and environmental sensitivity -->
  <extension domain="org.cognipilot.imu-stability" version="1.0">
    <imu-stability sensor="tactical_imu">
      <!-- From Allan variance analysis -->
      <accel-bias-stability unit="ug">3.6</accel-bias-stability>
      <gyro-bias-stability unit="deg/hr">2.0</gyro-bias-stability>

      <!-- Scale factor error (proportional measurement error) -->
      <accel-scale-factor-error unit="ppm">1500</accel-scale-factor-error>
      <gyro-scale-factor-error unit="ppm">100</gyro-scale-factor-error>

      <!-- Cross-axis sensitivity (leakage between axes) -->
      <cross-axis-sensitivity unit="percent">0.5</cross-axis-sensitivity>

      <!-- Gyro sensitivity to linear acceleration (vibration rectification) -->
      <gyro-g-sensitivity unit="deg/s/g">0.005</gyro-g-sensitivity>

      <!-- Angular random walk (noise density from Allan variance) -->
      <gyro-arw unit="deg/sqrt(hr)">0.16</gyro-arw>
      <accel-vrw unit="m/s/sqrt(hr)">0.012</accel-vrw>

      <!-- Temperature sensitivity -->
      <gyro-temp-sensitivity unit="deg/s/degC">0.005</gyro-temp-sensitivity>
      <accel-temp-sensitivity unit="mg/degC">0.1</accel-temp-sensitivity>
      <operating-temp unit="degC" min="-40" max="105"/>

      <!-- Allan variance test conditions -->
      <allan-test>
        <duration unit="hr">4</duration>
        <temperature unit="degC">25</temperature>
        <vibration>none</vibration>
      </allan-test>
    </imu-stability>
  </extension>

  <port name="spi0" iface="spi0" type="SPI">
    <capabilities>
      <speed unit="MHz">15</speed>
    </capabilities>
  </port>
</comp>
```

## How an Agent Uses This

A navigation engineer's agent would:

1. **Check if the extension exists:**
   ```
   Does comp have extension domain="org.cognipilot.imu-stability"?
   ```

2. **Extract stability parameters for filter tuning:**
   - `gyro-bias-stability` → sets the process noise floor for gyro bias state in EKF
   - `gyro-arw` → validates the core noise model's stddev is consistent
   - `gyro-g-sensitivity` → determines if vibration isolation is needed
     (if g-sensitivity × expected vibration g > acceptable drift rate)

3. **Predict dead-reckoning performance:**
   - With `accel-bias-stability=3.6ug` and `gyro-bias-stability=2.0 deg/hr`:
     position drift ≈ 0.5 × accel_bias × t² after GPS loss
   - At 3.6ug: ~0.17m drift after 10 minutes — acceptable for GPS-denied transit
   - A consumer IMU at 1000ug: ~47m drift after 10 minutes — not acceptable

4. **Validate operating environment:**
   - `operating-temp` vs expected environment
   - `gyro-g-sensitivity` vs expected vibration profile of the robot base

## Why Extension and Not Core

| Reason | Detail |
|--------|--------|
| **99% don't need it** | Mobile robots with GNSS re-acquire position within seconds |
| **Requires lab testing** | Allan variance parameters come from 4+ hour static tests, not datasheets alone |
| **Domain-specific** | Aerospace, defense, underwater — not general robotics |
| **Filter handles it** | Modern EKFs estimate bias online — they don't need Allan specs as inputs |
| **Datasheet values vary** | Allan variance is temperature and environment dependent — a single number is misleading |

## Extension Schema Convention

- **Domain:** `org.cognipilot.imu-stability` (reverse-DNS, avoids conflicts)
- **Version:** `1.0` (allows evolution without breaking existing files)
- **sensor attribute:** References the sensor name this stability data applies to
- **Units:** Always explicit via `unit` attribute (no defaults — these parameters
  have unusual units like `deg/sqrt(hr)` that shouldn't be assumed)

## Other Extension Ideas

The same pattern works for any domain-specific sensor data:

| Extension Domain | Use Case |
|-----------------|----------|
| `org.cognipilot.imu-stability` | Tactical IMU Allan variance |
| `org.cognipilot.camera-calibration` | Extrinsic calibration results, reprojection error |
| `org.cognipilot.lidar-multipath` | Multi-path rejection parameters for structured environments |
| `com.vendor.radar-cfar` | Radar CFAR detection threshold tuning |
| `org.ros.sensor-msgs` | ROS message type mapping for each sensor |

The core HCDF schema stays clean and focused. Domain expertise lives in
versioned extensions that agents can optionally understand.
