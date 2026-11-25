use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use rmk_config::resolved::hardware::{
    ChipSeries, CommunicationProtocol, DisplayConfig, DisplayDriver, I2cConfig, SpiConfig,
};

use super::input_device::Initializer;

/// Expand display configuration into initialization code + a processor.
/// Returns (initialization_code, processor_initializer).
pub(crate) fn expand_display_config(
    chip_series: &ChipSeries,
    display_config: &DisplayConfig,
) -> (TokenStream, Initializer) {
    let protocol_init = expand_protocol_init(chip_series, &display_config.protocol);
    let display_init = expand_display_driver_init(display_config);
    let processor_init = expand_display_processor_init(display_config);

    let initialization = quote! {
        #protocol_init
        #display_init
    };

    let processor = Initializer {
        initializer: processor_init,
        var_name: format_ident!("display_processor"),
    };

    (initialization, processor)
}

/// Generate interrupt binding tokens for the display bus.
pub(crate) fn expand_display_interrupt(
    chip_series: &ChipSeries,
    display_config: &DisplayConfig,
) -> TokenStream {
    match &display_config.protocol {
        CommunicationProtocol::I2c(i2c) => expand_i2c_interrupt(chip_series, i2c),
        CommunicationProtocol::Spi(spi) => {
            require_nrf_spi(chip_series);
            let instance = format_ident!("{}", spi.instance);
            quote! {
                #instance => ::embassy_nrf::spim::InterruptHandler<::embassy_nrf::peripherals::#instance>;
            }
        }
    }
}

fn require_nrf_spi(chip_series: &ChipSeries) {
    if *chip_series != ChipSeries::Nrf52 {
        panic!("SPI displays are only supported on nRF52 chips");
    }
}

fn expand_i2c_interrupt(chip_series: &ChipSeries, i2c: &I2cConfig) -> TokenStream {
    let instance = format_ident!("{}", i2c.instance);

    match chip_series {
        ChipSeries::Rp2040 => {
            let irq = format_ident!("{}_IRQ", i2c.instance);
            quote! {
                #irq => ::embassy_rp::i2c::InterruptHandler<::embassy_rp::peripherals::#instance>;
            }
        }
        ChipSeries::Nrf52 => {
            quote! {
                #instance => ::embassy_nrf::twim::InterruptHandler<::embassy_nrf::peripherals::#instance>;
            }
        }
        ChipSeries::Stm32 => {
            let ev_irq = format_ident!("{}_EV", i2c.instance);
            let er_irq = format_ident!("{}_ER", i2c.instance);
            quote! {
                #ev_irq => ::embassy_stm32::i2c::EventInterruptHandler<::embassy_stm32::peripherals::#instance>;
                #er_irq => ::embassy_stm32::i2c::ErrorInterruptHandler<::embassy_stm32::peripherals::#instance>;
            }
        }
        ChipSeries::Esp32 => {
            quote! {}
        }
    }
}

fn expand_protocol_init(chip_series: &ChipSeries, protocol: &CommunicationProtocol) -> TokenStream {
    match protocol {
        CommunicationProtocol::I2c(i2c) => expand_i2c_init(chip_series, i2c),
        CommunicationProtocol::Spi(spi) => expand_spi_init(chip_series, spi),
    }
}

fn expand_spi_init(chip_series: &ChipSeries, spi: &SpiConfig) -> TokenStream {
    require_nrf_spi(chip_series);
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

fn expand_i2c_init(chip_series: &ChipSeries, i2c: &I2cConfig) -> TokenStream {
    let instance = format_ident!("{}", i2c.instance.to_uppercase());
    let sda = format_ident!("{}", i2c.sda);
    let scl = format_ident!("{}", i2c.scl);

    match chip_series {
        ChipSeries::Rp2040 => {
            quote! {
                let display_i2c = ::embassy_rp::i2c::I2c::new(
                    p.#instance, p.#scl, p.#sda, Irqs, ::embassy_rp::i2c::Config::default()
                );
            }
        }
        ChipSeries::Nrf52 => {
            quote! {
                static DISPLAY_I2C_BUF: ::static_cell::StaticCell<[u8; 256]> = ::static_cell::StaticCell::new();
                let display_i2c_buf = DISPLAY_I2C_BUF.init([0u8; 256]);
                let display_i2c = ::embassy_nrf::twim::Twim::new(
                    p.#instance, Irqs, p.#sda, p.#scl, ::embassy_nrf::twim::Config::default(), display_i2c_buf
                );
            }
        }
        ChipSeries::Stm32 => {
            quote! {
                let display_i2c = ::embassy_stm32::i2c::I2c::new(
                    p.#instance, p.#scl, p.#sda, Irqs,
                    ::embassy_stm32::i2c::Config::default(),
                );
            }
        }
        ChipSeries::Esp32 => {
            // The display drivers need `embedded_hal_async::i2c::I2c`, which esp-hal implements for
            // the async driver only.
            quote! {
                let display_i2c = ::esp_hal::i2c::master::I2c::new(
                    p.#instance, ::esp_hal::i2c::master::Config::default()
                )
                .unwrap()
                .with_sda(p.#sda)
                .with_scl(p.#scl)
                .into_async();
            }
        }
    }
}

fn get_i2c_address(config: &DisplayConfig) -> u8 {
    match &config.protocol {
        CommunicationProtocol::I2c(i2c) => i2c.address,
        CommunicationProtocol::Spi(_) => panic!(
            "Display driver {:?} needs [display.protocol.i2c]",
            config.driver
        ),
    }
}

fn expand_display_driver_init(config: &DisplayConfig) -> TokenStream {
    // Index into each driver's rotation names below.
    let rotation = match config.rotation {
        0 => 0,
        90 => 1,
        180 => 2,
        270 => 3,
        rotation => panic!(
            "Unsupported display rotation '{}'. Supported: 0, 90, 180, 270",
            rotation
        ),
    };
    let oled_rotation = format_ident!(
        "{}",
        ["Rotate0", "Rotate90", "Rotate180", "Rotate270"][rotation]
    );

    match &config.driver {
        DisplayDriver::Ssd1306 => {
            let address = get_i2c_address(config);
            let display_size = parse_ssd1306_size(&config.size);
            quote! {
                let display_interface = ::rmk::display::ssd1306::I2CDisplayInterface::new_custom_address(display_i2c, #address);
                let display = ::rmk::display::ssd1306::Ssd1306Async::new(
                    display_interface,
                    #display_size,
                    ::rmk::display::ssd1306::prelude::DisplayRotation::#oled_rotation,
                ).into_buffered_graphics_mode();
            }
        }
        driver @ (DisplayDriver::Sh1106
        | DisplayDriver::Sh1107
        | DisplayDriver::Sh1108
        | DisplayDriver::Ssd1309) => {
            let address = get_i2c_address(config);
            let display_variant_path = parse_oled_async_variant(driver, &config.size);
            quote! {
                let display_interface = ::rmk::display::display_interface_i2c::I2CInterface::new(display_i2c, #address, 0x40);
                let display: ::rmk::display::oled_async::mode::graphics::GraphicsMode<_, _> =
                    ::rmk::display::oled_async::Builder::new(#display_variant_path {})
                        .with_rotation(::rmk::display::oled_async::displayrotation::DisplayRotation::#oled_rotation)
                        .connect(display_interface)
                        .into();
            }
        }
        DisplayDriver::Ls011b7dh03 => {
            if !matches!(config.protocol, CommunicationProtocol::Spi(_)) {
                panic!("Display driver Ls011b7dh03 needs [display.protocol.spi]");
            }
            if config.size != "160x68" {
                panic!(
                    "Unsupported LS011B7DH03 display size '{}'. Supported: 160x68",
                    config.size
                );
            }
            let rotation = format_ident!("{}", ["Deg0", "Deg90", "Deg180", "Deg270"][rotation]);
            quote! {
                let display = ::rmk::display::drivers::sharp_memory::Ls011b7dh03::new(
                    display_spi,
                    display_cs,
                    ::embassy_time::Delay,
                    ::rmk::display::drivers::sharp_memory::Rotation::#rotation,
                );
            }
        }
    }
}

/// Memory LCDs must toggle VCOM about once a second, which each flush does.
const MEMORY_LCD_RENDER_INTERVAL_MS: u64 = 1000;

fn expand_display_processor_init(config: &DisplayConfig) -> TokenStream {
    let constructor = if let Some(renderer_path) = &config.renderer {
        // Allow bare names like "OledRenderer" as shorthand for "::rmk::display::OledRenderer".
        let full_path = if renderer_path.contains("::") {
            renderer_path.clone()
        } else {
            format!("::rmk::display::{}", renderer_path)
        };
        let renderer_type: syn::Type = syn::parse_str(&full_path)
            .unwrap_or_else(|e| panic!("Invalid renderer type path '{}': {}", renderer_path, e));
        quote! {
            ::rmk::display::DisplayProcessor::with_renderer(
                display,
                #renderer_type::default(),
            )
        }
    } else {
        quote! {
            ::rmk::display::DisplayProcessor::new(display)
        }
    };

    let render_interval = match config.driver {
        DisplayDriver::Ls011b7dh03 => Some(
            config
                .render_interval
                .unwrap_or(MEMORY_LCD_RENDER_INTERVAL_MS),
        ),
        _ => config.render_interval,
    };
    let render_interval = render_interval.map(|ms| {
        quote! {
            .with_render_interval(::embassy_time::Duration::from_millis(#ms))
        }
    });

    let min_render_interval = config.min_render_interval.map(|ms| {
        quote! {
            .with_min_render_interval(::embassy_time::Duration::from_millis(#ms))
        }
    });

    quote! {
        let mut display_processor = #constructor
            #render_interval
            #min_render_interval;
    }
}

fn parse_ssd1306_size(size: &str) -> TokenStream {
    match size {
        "128x64" => quote! { ::rmk::display::ssd1306::prelude::DisplaySize128x64 },
        "128x32" => quote! { ::rmk::display::ssd1306::prelude::DisplaySize128x32 },
        "96x16" => quote! { ::rmk::display::ssd1306::prelude::DisplaySize96x16 },
        "72x40" => quote! { ::rmk::display::ssd1306::prelude::DisplaySize72x40 },
        "64x48" => quote! { ::rmk::display::ssd1306::prelude::DisplaySize64x48 },
        _ => panic!(
            "Unsupported SSD1306 display size '{}'. Supported: 128x64, 128x32, 96x16, 72x40, 64x48",
            size
        ),
    }
}

fn parse_oled_async_variant(driver: &DisplayDriver, size: &str) -> TokenStream {
    let (module, variant_name) = match (driver, size) {
        (DisplayDriver::Sh1106, "128x64") => ("sh1106", "Sh1106_128_64"),
        (DisplayDriver::Sh1107, "64x128") => ("sh1107", "Sh1107_64_128"),
        (DisplayDriver::Sh1107, "128x128") => ("sh1107", "Sh1107_128_128"),
        (DisplayDriver::Sh1108, "64x160") => ("sh1108", "Sh1108_64_160"),
        (DisplayDriver::Sh1108, "96x160") => ("sh1108", "Sh1108_96_160"),
        (DisplayDriver::Sh1108, "128x160") => ("sh1108", "Sh1108_128_160"),
        (DisplayDriver::Sh1108, "160x160") => ("sh1108", "Sh1108_160_160"),
        (DisplayDriver::Ssd1309, "128x64") => ("ssd1309", "Ssd1309_128_64"),
        (DisplayDriver::Sh1106, _) => {
            panic!(
                "Unsupported SH1106 display size '{}'. Supported: 128x64",
                size
            )
        }
        (DisplayDriver::Sh1107, _) => {
            panic!(
                "Unsupported SH1107 display size '{}'. Supported: 64x128, 128x128",
                size
            )
        }
        (DisplayDriver::Sh1108, _) => panic!(
            "Unsupported SH1108 display size '{}'. Supported: 64x160, 96x160, 128x160, 160x160",
            size
        ),
        (DisplayDriver::Ssd1309, _) => {
            panic!(
                "Unsupported SSD1309 display size '{}'. Supported: 128x64",
                size
            )
        }
        _ => unreachable!(),
    };

    let module_ident = format_ident!("{}", module);
    let variant_ident = format_ident!("{}", variant_name);

    quote! { ::rmk::display::oled_async::displays::#module_ident::#variant_ident }
}

#[cfg(test)]
mod tests {
    use rmk_config::resolved::hardware::SpiConfig;

    use super::*;

    fn nice_view(protocol: CommunicationProtocol, render_interval: Option<u64>) -> DisplayConfig {
        DisplayConfig {
            driver: DisplayDriver::Ls011b7dh03,
            protocol,
            size: "160x68".to_string(),
            rotation: 90,
            renderer: Some("OledRenderer".to_string()),
            render_interval,
            min_render_interval: None,
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
    #[should_panic(expected = "needs [display.protocol.i2c]")]
    fn oled_rejects_spi() {
        let config = DisplayConfig {
            driver: DisplayDriver::Ssd1306,
            size: "128x32".to_string(),
            ..nice_view(nice_view_spi(), None)
        };
        generated(ChipSeries::Nrf52, &config);
    }
}
