use rmk_config::{CommunicationProtocol, DisplayDriver, KeyboardTomlConfig};

#[test]
fn nice_view_display_resolves_without_miso() {
    let path = std::env::temp_dir().join(format!("rmk-nice-view-{}.toml", std::process::id()));
    std::fs::write(
        &path,
        r#"
[keyboard]
name = "RMK Test"
vendor_id = 0x4c4b
product_id = 0x4643
chip = "nrf52840"

[matrix]
row_pins = ["P0_02"]
col_pins = ["P0_03"]

[layout]
rows = 1
cols = 1

[display]
driver = "nice_view"
size = "160x68"
rotation = 90

[display.protocol.spi]
instance = "SPI2"
sck = "P0_20"
mosi = "P0_17"
cs = "P0_06"
"#,
    )
    .unwrap();
    let config = KeyboardTomlConfig::new_from_toml_path(&path);
    std::fs::remove_file(path).unwrap();

    let display = config.hardware().unwrap().display.unwrap();
    assert!(matches!(display.driver, DisplayDriver::Ls011b7dh03));
    let CommunicationProtocol::Spi(spi) = display.protocol else {
        panic!("expected an SPI display");
    };
    assert_eq!(spi.miso, "");
    assert_eq!(spi.cs.as_deref(), Some("P0_06"));
}
