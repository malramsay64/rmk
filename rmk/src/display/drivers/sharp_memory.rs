//! [`DisplayDriver`] for the Sharp LS011B7DH03 memory-in-pixel LCD used by the
//! [nice!view](https://nicekeyboards.com/nice-view).
//!
//! The panel is 160x68, write-only, and driven over SPI (mode 0, MSB first) with
//! an active-high chip select. Bits are reversed in software, so the bus needs no
//! LSB-first support.
//!
//! # Example
//!
//! ```rust,ignore
//! use rmk::display::DisplayProcessor;
//! use rmk::display::drivers::sharp_memory::{Ls011b7dh03, Rotation};
//!
//! let mut config = embassy_nrf::spim::Config::default();
//! config.frequency = embassy_nrf::spim::Frequency::M1;
//! let spi = embassy_nrf::spim::Spim::new_txonly(p.SPI2, p.P0_20, p.P0_17, Irqs, config);
//! let cs = embassy_nrf::gpio::Output::new(p.P0_06, Level::Low, OutputDrive::Standard);
//! let display = Ls011b7dh03::new(spi, cs, embassy_time::Delay, Rotation::Deg90);
//! // The panel needs its VCOM toggled about once a second, which every flush does.
//! let mut processor = DisplayProcessor::new(display).with_render_interval(Duration::from_secs(1));
//! ```

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::spi::SpiBus;

use super::super::DisplayDriver;

const WIDTH: usize = 160;
const HEIGHT: usize = 68;
const LINE_BYTES: usize = WIDTH / 8;

// Command bits as sent MSB first; the panel reads them LSB first.
const CMD_WRITE: u8 = 0x80;
const CMD_VCOM: u8 = 0x40;
const CMD_CLEAR: u8 = 0x20;

// Chip select setup and hold times from the LS011B7DH03 datasheet.
const CS_SETUP_US: u32 = 6;
const CS_HOLD_US: u32 = 2;
// Settle time after switching panel power on, before the first command.
const POWER_UP_MS: u32 = 10;

/// Power pin for a panel that is always powered.
pub struct NoPowerPin;

impl embedded_hal::digital::ErrorType for NoPowerPin {
    type Error = core::convert::Infallible;
}

impl OutputPin for NoPowerPin {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Display rotation, clockwise. 90 and 270 give a 68x160 portrait canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

/// Sharp LS011B7DH03 memory LCD (nice!view).
///
/// [`BinaryColor::On`] draws a dark pixel on the reflective (light) background.
/// [`flush`](DisplayDriver::flush) sends only the lines drawn since the last
/// flush, and toggles VCOM every time it runs, even when nothing changed.
///
/// With a power pin (active high, see [`with_power_pin`](Self::with_power_pin)),
/// the panel is switched off while the keyboard sleeps and cleared on wake.
pub struct Ls011b7dh03<SPI, CS, D, PWR = NoPowerPin> {
    spi: SPI,
    cs: CS,
    delay: D,
    power: PWR,
    powered: bool,
    rotation: Rotation,
    vcom: bool,
    /// Wire-order framebuffer: bit 7 of byte 0 is the leftmost pixel, 1 = light.
    buffer: [[u8; LINE_BYTES]; HEIGHT],
    /// Bit `n` set = line `n` changed since the last flush.
    dirty: u128,
}

impl<SPI, CS, D> Ls011b7dh03<SPI, CS, D> {
    /// `cs` must start low (deselected).
    pub fn new(spi: SPI, cs: CS, delay: D, rotation: Rotation) -> Self {
        Self {
            spi,
            cs,
            delay,
            power: NoPowerPin,
            powered: true,
            rotation,
            vcom: false,
            buffer: [[0xFF; LINE_BYTES]; HEIGHT],
            dirty: 0,
        }
    }

    /// Switch panel power with `power`, which must start high (on).
    pub fn with_power_pin<P: OutputPin>(self, power: P) -> Ls011b7dh03<SPI, CS, D, P> {
        Ls011b7dh03 {
            spi: self.spi,
            cs: self.cs,
            delay: self.delay,
            power,
            powered: self.powered,
            rotation: self.rotation,
            vcom: self.vcom,
            buffer: self.buffer,
            dirty: self.dirty,
        }
    }
}

impl<SPI, CS, D, PWR> Ls011b7dh03<SPI, CS, D, PWR>
where
    SPI: SpiBus,
    CS: OutputPin,
    D: DelayNs,
    PWR: OutputPin,
{
    /// Send `frame` with the chip selected.
    async fn send(&mut self, frame: &[&[u8]]) -> Result<(), SPI::Error> {
        self.cs.set_high().ok();
        self.delay.delay_us(CS_SETUP_US).await;
        let mut result = Ok(());
        for part in frame {
            result = self.spi.write(part).await;
            if result.is_err() {
                break;
            }
        }
        if result.is_ok() {
            result = self.spi.flush().await;
        }
        self.delay.delay_us(CS_HOLD_US).await;
        self.cs.set_low().ok();
        result
    }

    fn vcom_bit(&mut self) -> u8 {
        self.vcom = !self.vcom;
        if self.vcom { CMD_VCOM } else { 0 }
    }
}

impl<SPI, CS, D, PWR> DisplayDriver for Ls011b7dh03<SPI, CS, D, PWR>
where
    SPI: SpiBus,
    CS: OutputPin,
    D: DelayNs,
    PWR: OutputPin,
{
    async fn init(&mut self) {
        let cmd = CMD_CLEAR | self.vcom_bit();
        if self.send(&[&[cmd, 0x00]]).await.is_ok() {
            self.buffer = [[0xFF; LINE_BYTES]; HEIGHT];
            self.dirty = 0;
        }
    }

    async fn flush(&mut self) {
        // Driving the bus into an unpowered panel can back-power it through its inputs.
        if !self.powered {
            return;
        }
        let vcom = self.vcom_bit();
        if self.dirty == 0 {
            // Nothing to draw: send only the VCOM toggle.
            self.send(&[&[vcom, 0x00]]).await.ok();
            return;
        }

        self.cs.set_high().ok();
        self.delay.delay_us(CS_SETUP_US).await;
        let mut ok = self.spi.write(&[CMD_WRITE | vcom]).await.is_ok();
        for y in 0..HEIGHT {
            if !ok {
                break;
            }
            if self.dirty & (1 << y) == 0 {
                continue;
            }
            // Line address (1-based), the pixels, then a dummy byte.
            let mut line = [0u8; LINE_BYTES + 2];
            line[0] = (y as u8 + 1).reverse_bits();
            line[1..=LINE_BYTES].copy_from_slice(&self.buffer[y]);
            ok = self.spi.write(&line).await.is_ok();
        }
        // Trailing dummy byte ends the transfer.
        ok = ok && self.spi.write(&[0x00]).await.is_ok() && self.spi.flush().await.is_ok();
        self.delay.delay_us(CS_HOLD_US).await;
        self.cs.set_low().ok();

        if ok {
            self.dirty = 0;
        }
    }

    async fn set_sleeping(&mut self, sleeping: bool) {
        if sleeping != self.powered {
            return;
        }
        self.powered = !sleeping;
        if sleeping {
            self.power.set_low().ok();
        } else {
            self.power.set_high().ok();
            self.delay.delay_ms(POWER_UP_MS).await;
            // The panel lost its image while off.
            self.init().await;
        }
    }
}

impl<SPI, CS, D, PWR> OriginDimensions for Ls011b7dh03<SPI, CS, D, PWR> {
    fn size(&self) -> Size {
        match self.rotation {
            Rotation::Deg0 | Rotation::Deg180 => Size::new(WIDTH as u32, HEIGHT as u32),
            Rotation::Deg90 | Rotation::Deg270 => Size::new(HEIGHT as u32, WIDTH as u32),
        }
    }
}

impl<SPI, CS, D, PWR> DrawTarget for Ls011b7dh03<SPI, CS, D, PWR> {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<BinaryColor>>,
    {
        let size = self.size();
        for Pixel(point, color) in pixels {
            let (x, y) = (point.x, point.y);
            if x < 0 || y < 0 || x >= size.width as i32 || y >= size.height as i32 {
                continue;
            }
            let (x, y) = (x as usize, y as usize);
            let (px, py) = match self.rotation {
                Rotation::Deg0 => (x, y),
                Rotation::Deg90 => (WIDTH - 1 - y, x),
                Rotation::Deg180 => (WIDTH - 1 - x, HEIGHT - 1 - y),
                Rotation::Deg270 => (y, HEIGHT - 1 - x),
            };
            let mask = 0x80 >> (px % 8);
            let byte = &mut self.buffer[py][px / 8];
            let old = *byte;
            match color {
                BinaryColor::On => *byte &= !mask,
                BinaryColor::Off => *byte |= mask,
            }
            if *byte != old {
                self.dirty |= 1 << py;
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: BinaryColor) -> Result<(), Self::Error> {
        let fill = match color {
            BinaryColor::On => 0x00,
            BinaryColor::Off => 0xFF,
        };
        for (y, line) in self.buffer.iter_mut().enumerate() {
            if line.iter().any(|&b| b != fill) {
                *line = [fill; LINE_BYTES];
                self.dirty |= 1 << y;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::vec;
    use std::vec::Vec;

    use embassy_futures::block_on;
    use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

    use super::*;

    /// Records the bytes sent during each chip-select frame.
    #[derive(Clone, Default)]
    struct Log(Rc<RefCell<Vec<Vec<u8>>>>);

    struct Bus(Log);
    struct Cs(Log, bool);
    struct NoDelay;
    struct Power(Rc<Cell<bool>>);

    impl embedded_hal_async::spi::ErrorType for Bus {
        type Error = core::convert::Infallible;
    }

    impl SpiBus for Bus {
        async fn read(&mut self, _: &mut [u8]) -> Result<(), Self::Error> {
            unreachable!()
        }
        async fn write(&mut self, words: &[u8]) -> Result<(), Self::Error> {
            self.0
                .0
                .borrow_mut()
                .last_mut()
                .expect("write with CS low")
                .extend_from_slice(words);
            Ok(())
        }
        async fn transfer(&mut self, _: &mut [u8], _: &[u8]) -> Result<(), Self::Error> {
            unreachable!()
        }
        async fn transfer_in_place(&mut self, _: &mut [u8]) -> Result<(), Self::Error> {
            unreachable!()
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    impl embedded_hal::digital::ErrorType for Cs {
        type Error = core::convert::Infallible;
    }

    impl OutputPin for Cs {
        fn set_high(&mut self) -> Result<(), Self::Error> {
            assert!(!self.1, "CS already high");
            self.1 = true;
            self.0.0.borrow_mut().push(Vec::new());
            Ok(())
        }
        fn set_low(&mut self) -> Result<(), Self::Error> {
            self.1 = false;
            Ok(())
        }
    }

    impl embedded_hal::digital::ErrorType for Power {
        type Error = core::convert::Infallible;
    }

    impl OutputPin for Power {
        fn set_high(&mut self) -> Result<(), Self::Error> {
            self.0.set(true);
            Ok(())
        }
        fn set_low(&mut self) -> Result<(), Self::Error> {
            self.0.set(false);
            Ok(())
        }
    }

    impl DelayNs for NoDelay {
        async fn delay_ns(&mut self, _: u32) {}
    }

    fn display(rotation: Rotation) -> (Ls011b7dh03<Bus, Cs, NoDelay>, Log) {
        let log = Log::default();
        let display = Ls011b7dh03::new(Bus(log.clone()), Cs(log.clone(), false), NoDelay, rotation);
        (display, log)
    }

    type PoweredDisplay = Ls011b7dh03<Bus, Cs, NoDelay, Power>;

    fn powered_display() -> (PoweredDisplay, Log, Rc<Cell<bool>>) {
        let (display, log) = display(Rotation::Deg0);
        let power = Rc::new(Cell::new(true));
        (display.with_power_pin(Power(power.clone())), log, power)
    }

    fn frames(log: &Log) -> Vec<Vec<u8>> {
        log.0.take()
    }

    /// Expected bytes for one line: address, pixels, dummy.
    fn line(y: u8, pixels: [u8; LINE_BYTES]) -> Vec<u8> {
        let mut bytes = vec![(y + 1).reverse_bits()];
        bytes.extend_from_slice(&pixels);
        bytes.push(0x00);
        bytes
    }

    #[test]
    fn init_clears_the_panel() {
        let (mut display, log) = display(Rotation::Deg0);
        block_on(display.init());
        assert_eq!(frames(&log), vec![vec![CMD_CLEAR | CMD_VCOM, 0x00]]);
    }

    #[test]
    fn flush_without_changes_only_toggles_vcom() {
        let (mut display, log) = display(Rotation::Deg0);
        block_on(display.flush());
        block_on(display.flush());
        assert_eq!(frames(&log), vec![vec![CMD_VCOM, 0x00], vec![0x00, 0x00]]);
    }

    #[test]
    fn flush_sends_only_changed_lines() {
        let (mut display, log) = display(Rotation::Deg0);
        Pixel(Point::new(0, 2), BinaryColor::On).draw(&mut display).unwrap();
        Pixel(Point::new(159, 67), BinaryColor::On).draw(&mut display).unwrap();
        block_on(display.flush());

        let mut first = [0xFF; LINE_BYTES];
        first[0] = 0x7F;
        let mut last = [0xFF; LINE_BYTES];
        last[LINE_BYTES - 1] = 0xFE;
        let mut expected = vec![CMD_WRITE | CMD_VCOM];
        expected.extend(line(2, first));
        expected.extend(line(67, last));
        expected.push(0x00);
        assert_eq!(frames(&log), vec![expected]);

        // Lines already sent are not sent again.
        block_on(display.flush());
        assert_eq!(frames(&log), vec![vec![0x00, 0x00]]);
    }

    #[test]
    fn redrawing_the_same_pixels_sends_nothing() {
        let (mut display, log) = display(Rotation::Deg0);
        display.clear(BinaryColor::Off).unwrap();
        Pixel(Point::new(5, 5), BinaryColor::Off).draw(&mut display).unwrap();
        block_on(display.flush());
        assert_eq!(frames(&log), vec![vec![CMD_VCOM, 0x00]]);
    }

    #[test]
    fn clear_marks_every_line() {
        let (mut display, log) = display(Rotation::Deg0);
        display.clear(BinaryColor::On).unwrap();
        block_on(display.flush());
        let frame = &frames(&log)[0];
        assert_eq!(frame.len(), 1 + HEIGHT * (LINE_BYTES + 2) + 1);
        assert_eq!(&frame[1..1 + LINE_BYTES + 2], line(0, [0x00; LINE_BYTES]).as_slice());
    }

    #[test]
    fn rotation_maps_corners() {
        // (rotation, logical point, physical line, byte, mask)
        let cases = [
            (Rotation::Deg0, Point::new(0, 0), 0, 0, 0x80),
            (Rotation::Deg90, Point::new(0, 0), 0, LINE_BYTES - 1, 0x01),
            (Rotation::Deg90, Point::new(67, 159), 67, 0, 0x80),
            (Rotation::Deg180, Point::new(0, 0), 67, LINE_BYTES - 1, 0x01),
            (Rotation::Deg270, Point::new(0, 0), 67, 0, 0x80),
        ];
        for (rotation, point, py, byte, mask) in cases {
            let (mut display, _) = display(rotation);
            Pixel(point, BinaryColor::On).draw(&mut display).unwrap();
            assert_eq!(display.buffer[py][byte], !mask, "{rotation:?} {point:?}");
            assert_eq!(display.dirty, 1 << py, "{rotation:?} {point:?}");
        }
    }

    #[test]
    fn portrait_rotation_swaps_size_and_clips() {
        let (mut display, _) = display(Rotation::Deg90);
        assert_eq!(display.size(), Size::new(68, 160));
        Rectangle::new(Point::new(60, 150), Size::new(20, 20))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut display)
            .unwrap();
        // Logical x 60..67, y 150..159 lands on physical lines 60..67, columns 0..9.
        assert_eq!(display.dirty, ((1u128 << 8) - 1) << 60);
        assert_eq!(display.buffer[60][0], 0x00);
        assert_eq!(display.buffer[60][1], 0x3F);
    }

    #[test]
    fn sleep_cuts_power_and_skips_flushes() {
        let (mut display, log, power) = powered_display();
        block_on(display.set_sleeping(true));
        assert!(!power.get());
        Pixel(Point::new(0, 0), BinaryColor::On).draw(&mut display).unwrap();
        block_on(display.flush());
        assert!(frames(&log).is_empty());
    }

    #[test]
    fn wake_powers_up_clears_and_redraws() {
        let (mut display, log, power) = powered_display();
        Pixel(Point::new(0, 0), BinaryColor::On).draw(&mut display).unwrap();
        block_on(display.flush());
        block_on(display.set_sleeping(true));
        frames(&log);

        block_on(display.set_sleeping(false));
        assert!(power.get());
        assert_eq!(frames(&log), vec![vec![CMD_CLEAR, 0x00]]);

        // The panel was blanked, so the same pixel is sent again.
        Pixel(Point::new(0, 0), BinaryColor::On).draw(&mut display).unwrap();
        block_on(display.flush());
        let mut pixels = [0xFF; LINE_BYTES];
        pixels[0] = 0x7F;
        let mut expected = vec![CMD_WRITE | CMD_VCOM];
        expected.extend(line(0, pixels));
        expected.push(0x00);
        assert_eq!(frames(&log), vec![expected]);
    }

    #[test]
    fn repeated_sleep_state_is_ignored() {
        let (mut display, log, power) = powered_display();
        block_on(display.set_sleeping(false));
        assert!(power.get());
        assert!(frames(&log).is_empty());
    }
}
