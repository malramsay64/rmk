//! Codegen for the Sharp LS011B7DH03 memory LCD (nice!view) over SPI on nRF52.
//!
//! Kept apart from `display.rs`, which only calls in here, so that file stays
//! close to upstream.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use rmk_config::resolved::hardware::{
    ChipSeries, CommunicationProtocol, DisplayConfig, DisplayDriver, SpiConfig,
};

/// Memory LCDs must toggle VCOM about once a second, which each flush does.
pub(crate) fn default_render_interval(driver: &DisplayDriver) -> Option<u64> {
    matches!(driver, DisplayDriver::Ls011b7dh03).then_some(1000)
}

pub(crate) fn expand_spi_interrupt(chip_series: &ChipSeries, spi: &SpiConfig) -> TokenStream {
    require_nrf(chip_series);
    let instance = format_ident!("{}", spi.instance);
    quote! {
        #instance => ::embassy_nrf::spim::InterruptHandler<::embassy_nrf::peripherals::#instance>;
    }
}

pub(crate) fn expand_spi_init(chip_series: &ChipSeries, spi: &SpiConfig) -> TokenStream {
    require_nrf(chip_series);
    let instance = format_ident!("{}", spi.instance);
    let sck = format_ident!("{}", spi.sck);
    let mosi = format_ident!("{}", spi.mosi);
    let cs = format_ident!(
        "{}",
        spi.cs
            .as_deref()
            .expect("SPI displays need `cs` in [display.protocol.spi]")
    );
    quote! {
        let mut display_spi_config = ::embassy_nrf::spim::Config::default();
        display_spi_config.frequency = ::embassy_nrf::spim::Frequency::M1;
        let display_spi = ::embassy_nrf::spim::Spim::new_txonly(
            p.#instance, p.#sck, p.#mosi, Irqs, display_spi_config
        );
        // The panel's chip select is active high.
        let display_cs = ::embassy_nrf::gpio::Output::new(
            p.#cs, ::embassy_nrf::gpio::Level::Low, ::embassy_nrf::gpio::OutputDrive::Standard
        );
    }
}

pub(crate) fn expand_ls011b7dh03_init(config: &DisplayConfig) -> TokenStream {
    if !matches!(config.protocol, CommunicationProtocol::Spi(_)) {
        panic!("Display driver ls011b7dh03 needs [display.protocol.spi]");
    }
    if config.size != "160x68" {
        panic!(
            "Unsupported LS011B7DH03 display size '{}'. Supported: 160x68",
            config.size
        );
    }
    let rotation = match config.rotation {
        0 => quote! { Deg0 },
        90 => quote! { Deg90 },
        180 => quote! { Deg180 },
        270 => quote! { Deg270 },
        rotation => panic!(
            "Unsupported display rotation '{}'. Supported: 0, 90, 180, 270",
            rotation
        ),
    };
    let power = config.power_pin.as_ref().map(|pin| {
        let pin = format_ident!("{}", pin);
        quote! {
            .with_power_pin(::embassy_nrf::gpio::Output::new(
                p.#pin, ::embassy_nrf::gpio::Level::High, ::embassy_nrf::gpio::OutputDrive::Standard
            ))
        }
    });
    quote! {
        let display = ::rmk::display::drivers::sharp_memory::Ls011b7dh03::new(
            display_spi,
            display_cs,
            ::embassy_time::Delay,
            ::rmk::display::drivers::sharp_memory::Rotation::#rotation,
        )#power;
    }
}

/// `power_pin` is only wired up for this driver; reject it elsewhere instead of ignoring it.
pub(crate) fn reject_power_pin(config: &DisplayConfig) {
    if config.power_pin.is_some() {
        panic!("`power_pin` is only supported by the ls011b7dh03 display driver");
    }
}

fn require_nrf(chip_series: &ChipSeries) {
    if *chip_series != ChipSeries::Nrf52 {
        panic!("SPI displays are only supported on nRF52 chips");
    }
}

#[cfg(test)]
mod tests {
    use rmk_config::resolved::hardware::SpiConfig;

    use super::*;
    use crate::codegen::display::{expand_display_config, expand_display_interrupt};
    use rmk_config::resolved::hardware::I2cConfig;

    fn nice_view(protocol: CommunicationProtocol, render_interval: Option<u64>) -> DisplayConfig {
        DisplayConfig {
            driver: DisplayDriver::Ls011b7dh03,
            protocol,
            size: "160x68".to_string(),
            rotation: 90,
            renderer: Some("OledRenderer".to_string()),
            render_interval,
            min_render_interval: None,
            power_pin: None,
        }
    }

    fn nice_view_spi() -> CommunicationProtocol {
        CommunicationProtocol::Spi(SpiConfig {
            instance: "SPI2".to_string(),
            sck: "P0_20".to_string(),
            mosi: "P0_17".to_string(),
            cs: Some("P0_06".to_string()),
            ..Default::default()
        })
    }

    fn generated(chip: ChipSeries, config: &DisplayConfig) -> String {
        let (init, processor) = expand_display_config(&chip, config);
        let interrupt = expand_display_interrupt(&chip, config);
        format!("{init} {} {interrupt}", processor.initializer)
    }

    #[test]
    fn nice_view_uses_tx_only_spi_and_polls_for_vcom() {
        let code = generated(ChipSeries::Nrf52, &nice_view(nice_view_spi(), None));
        assert!(code.contains("Spim :: new_txonly (p . SPI2 , p . P0_20 , p . P0_17 , Irqs"));
        assert!(code.contains("Output :: new (p . P0_06 , :: embassy_nrf :: gpio :: Level :: Low"));
        assert!(code.contains("Ls011b7dh03 :: new"));
        assert!(code.contains("Rotation :: Deg90"));
        assert!(code.contains("from_millis (1000u64)"));
        assert!(code.contains("SPI2 => :: embassy_nrf :: spim :: InterruptHandler"));
    }

    #[test]
    fn nice_view_switches_its_power_pin() {
        let mut config = nice_view(nice_view_spi(), None);
        let code = generated(ChipSeries::Nrf52, &config);
        assert!(!code.contains("with_power_pin"));
        config.power_pin = Some("P0_13".to_string());
        let code = generated(ChipSeries::Nrf52, &config);
        assert!(code.contains(
            "with_power_pin (:: embassy_nrf :: gpio :: Output :: new (p . P0_13 , :: embassy_nrf :: gpio :: Level :: High"
        ));
    }

    #[test]
    #[should_panic(expected = "only supported by the ls011b7dh03")]
    fn oled_rejects_power_pin() {
        let i2c = I2cConfig {
            instance: "TWISPI0".to_string(),
            sda: "P0_17".to_string(),
            scl: "P0_20".to_string(),
            address: 0x3C,
        };
        let config = DisplayConfig {
            driver: DisplayDriver::Ssd1306,
            size: "128x32".to_string(),
            power_pin: Some("P0_13".to_string()),
            ..nice_view(CommunicationProtocol::I2c(i2c), None)
        };
        generated(ChipSeries::Nrf52, &config);
    }

    #[test]
    fn nice_view_keeps_an_explicit_render_interval() {
        let code = generated(ChipSeries::Nrf52, &nice_view(nice_view_spi(), Some(250)));
        assert!(code.contains("from_millis (250u64)"));
        assert!(!code.contains("from_millis (1000u64)"));
    }

    #[test]
    #[should_panic(expected = "needs [display.protocol.spi]")]
    fn nice_view_rejects_i2c() {
        let i2c = I2cConfig {
            instance: "TWISPI0".to_string(),
            sda: "P0_17".to_string(),
            scl: "P0_20".to_string(),
            address: 0x3C,
        };
        let config = nice_view(CommunicationProtocol::I2c(i2c), None);
        generated(ChipSeries::Nrf52, &config);
    }

    #[test]
    #[should_panic(expected = "only supported on nRF52")]
    fn spi_display_rejects_other_chips() {
        generated(ChipSeries::Rp2040, &nice_view(nice_view_spi(), None));
    }

    #[test]
    #[should_panic(expected = "SPI display interface is not yet supported")]
    fn oled_rejects_spi() {
        let config = DisplayConfig {
            driver: DisplayDriver::Ssd1306,
            size: "128x32".to_string(),
            ..nice_view(nice_view_spi(), None)
        };
        generated(ChipSeries::Nrf52, &config);
    }
}
