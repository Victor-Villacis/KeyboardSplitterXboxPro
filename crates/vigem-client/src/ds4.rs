use std::{fmt, mem, ptr};
use std::borrow::Borrow;
#[cfg(feature = "unstable_ds4")]
use std::{thread, time};
#[cfg(feature = "unstable_ds4")]
use winapi::shared::winerror;
use crate::*;

/// How long [`DualShock4Wired::wait_ready`] keeps offering a neutral report
/// before it gives up on the HID stack ever polling this target.
///
/// Measured window on ViGEmBus 1.21.442.0 is 1-3ms; a second is pure headroom.
#[cfg(feature = "unstable_ds4")]
const READY_BUDGET: time::Duration = time::Duration::from_secs(1);

/// How long [`DualShock4Wired::update`] retries a report the driver currently
/// has nowhere to put. Short: `wait_ready` already absorbed the startup window,
/// so this only covers a transient, and it sits in the caller's input path.
#[cfg(feature = "unstable_ds4")]
const UPDATE_BUDGET: time::Duration = time::Duration::from_millis(2);

/// Gap between retries. The driver flushes its pending interrupt-IN queue every
/// 5ms (`DS4_QUEUE_FLUSH_PERIOD`, ViGEmBus sys/Ds4Pdo.hpp), so retrying at a
/// coarser grain than that would just waste the budget.
#[cfg(feature = "unstable_ds4")]
const RETRY_GAP: time::Duration = time::Duration::from_micros(250);

/// DualShock4 HID Input report.
#[cfg(feature = "unstable_ds4")]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct DS4Report {
	pub thumb_lx: u8,
	pub thumb_ly: u8,
	pub thumb_rx: u8,
	pub thumb_ry: u8,
	pub buttons: u16,
	pub special: u8,
	pub trigger_l: u8,
	pub trigger_r: u8,
}
#[cfg(feature = "unstable_ds4")]
impl Default for DS4Report {
	#[inline]
	fn default() -> Self {
		DS4Report {
			thumb_lx: 0x80,
			thumb_ly: 0x80,
			thumb_rx: 0x80,
			thumb_ry: 0x80,
			buttons: 0x8,
			special: 0,
			trigger_l: 0,
			trigger_r: 0,
		}
	}
}

// /// DualShock4 v1 complete HID Input report.
// #[derive(Copy, Clone, Debug, Eq, PartialEq)]
// #[repr(C)]
// pub struct DS4ReportEx {
// 	pub thumb_lx: u8,
// 	pub thumb_ly: u8,
// 	pub thumb_rx: u8,
// 	pub thumb_ry: u8,
// 	pub buttons: u16,
// 	pub special: u8,
// 	pub trigger_l: u8,
// 	pub trigger_r: u8,
// 	pub timestamp: u16,
// 	pub battery_lvl: u8,
// 	pub gyro_x: i16,
// 	pub gyro_y: i16,
// 	pub gyro_z: i16,
// 	pub accel_x: i16,
// 	pub accel_y: i16,
// 	pub accel_z: i16,
// 	pub _unknown1: [u8; 5],
// 	pub battery_lvl_special: u8,
// 	pub _unknown2: [u8; 2],
// 	pub touch_packets_n: u8, // 0x00 to 0x03 (USB max)
// 	pub current_touch: DS4Touch,
// 	pub previous_touch: [DS4Touch; 2],
// }

/// A virtual Sony DualShock 4 (wired).
pub struct DualShock4Wired<CL: Borrow<Client>> {
	client: CL,
	event: Event,
	serial_no: u32,
	id: TargetId,
}

impl<CL: Borrow<Client>> DualShock4Wired<CL> {
	/// Creates a new instance.
	#[inline]
	pub fn new(client: CL, id: TargetId) -> DualShock4Wired<CL> {
		let event = Event::new(false, false);
		DualShock4Wired { client, event, serial_no: 0, id }
	}

	/// Returns if the controller is plugged in.
	#[inline]
	pub fn is_attached(&self) -> bool {
		self.serial_no != 0
	}

	/// Returns the id the controller was constructed with.
	#[inline]
	pub fn id(&self) -> TargetId {
		self.id
	}

	/// Returns the client.
	#[inline]
	pub fn client(&self) -> &CL {
		&self.client
	}

	/// Unplugs and destroys the controller, returning the client.
	#[inline]
	pub fn drop(mut self) -> CL {
		let _ = self.unplug();

		unsafe {
			let client = (&self.client as *const CL).read();
			ptr::drop_in_place(&mut self.event);
			mem::forget(self);
			client
		}
	}

	/// Plugs the controller in.
	#[inline(never)]
	pub fn plugin(&mut self) -> Result<(), Error> {
		if self.is_attached() {
			return Err(Error::AlreadyConnected);
		}

		self.serial_no = unsafe {
			let mut plugin = bus::PluginTarget::ds4_wired(1, self.id.vendor, self.id.product);
			let device = self.client.borrow().device;

			// Yes this is how the driver is implemented
			while plugin.ioctl(device, self.event.handle).is_err() {
				plugin.SerialNo += 1;
				if plugin.SerialNo >= u16::MAX as u32 {
					return Err(Error::NoFreeSlot);
				}
			}

			plugin.SerialNo
		};

		Ok(())
	}

	/// Unplugs the controller.
	#[inline(never)]
	pub fn unplug(&mut self) -> Result<(), Error> {
		if !self.is_attached() {
			return Err(Error::NotPluggedIn);
		}

		unsafe {
			let mut unplug = bus::UnplugTarget::new(self.serial_no);
			let device = self.client.borrow().device;
			unplug.ioctl(device, self.event.handle)?;
		}

		self.serial_no = 0;
		Ok(())
	}

	/// Waits until the virtual controller is ready.
	///
	/// Any updates submitted before the virtual controller is ready may return an error.
	#[inline(never)]
	pub fn wait_ready(&mut self) -> Result<(), Error> {
		if !self.is_attached() {
			return Err(Error::NotPluggedIn);
		}

		unsafe {
			let mut wait = bus::WaitDeviceReady::new(self.serial_no);
			let device = self.client.borrow().device;
			wait.ioctl(device, self.event.handle)?;
		}

		// IOCTL_WAIT_DEVICE_READY only says the bus finished creating the PDO.
		// A DS4 target needs more than that before it will take a report:
		// `EmulationTargetDS4::SubmitReportImpl` (ViGEmBus sys/Ds4Pdo.cpp) opens
		// by dequeuing a pending interrupt-IN request and returns that queue's
		// status when there is none -- STATUS_NO_MORE_ENTRIES, which surfaces to
		// user mode as ERROR_NO_MORE_ITEMS (259). Those requests only exist once
		// the HID stack above the PDO starts polling, which lags this IOCTL by
		// 1-3ms. Absorb that window here so the caller's first real update is
		// not dropped on the floor.
		//
		// X360 needs none of this: xusb22 keeps the interrupt-IN queue populated
		// from the moment the device starts, which is why the identically shaped
		// `Xbox360Wired::update` has never been seen to fail this way.
		#[cfg(feature = "unstable_ds4")]
		self.submit(&DS4Report::default(), READY_BUDGET)?;

		Ok(())
	}

	/// Submits one report, retrying while the driver has no pending
	/// interrupt-IN request to complete (`ERROR_NO_MORE_ITEMS`).
	///
	/// A refused submit is a *dropped* report: `SubmitReportImpl` returns before
	/// it copies anything into the PDO's cached report, so the state change is
	/// simply lost. Upstream ViGEmClient hides this -- `vigem_target_ds4_update`
	/// ignores every `GetOverlappedResult` failure except `ERROR_ACCESS_DENIED`
	/// and reports `VIGEM_ERROR_NONE` -- which is harmless for a client that
	/// streams reports on a timer (DS4Windows) and wrong for one that submits
	/// only on change. Retrying is what actually gets the report to the driver.
	#[cfg(feature = "unstable_ds4")]
	fn submit(&mut self, report: &DS4Report, budget: time::Duration) -> Result<(), Error> {
		let start = time::Instant::now();
		loop {
			let result = unsafe {
				let mut dsr = bus::DS4SubmitReport::new(self.serial_no, *report);
				let device = self.client.borrow().device;
				dsr.ioctl(device, self.event.handle)
			};
			match result {
				Ok(()) => return Ok(()),
				// Nothing is polling the target yet (or right now).
				Err(winerror::ERROR_NO_MORE_ITEMS) => {
					if start.elapsed() >= budget {
						return Err(Error::TargetNotReady);
					}
					thread::sleep(RETRY_GAP);
				}
				// Bus_Ds4SubmitReportHandler could not find this serial.
				Err(winerror::ERROR_DEV_NOT_EXIST) => return Err(Error::TargetNotReady),
				Err(err) => return Err(Error::WinError(err)),
			}
		}
	}

	/// Updates the virtual controller state.
	///
	/// Returns [`Error::TargetNotReady`] if the driver had nowhere to put the
	/// report for the whole retry budget, meaning it was *not* delivered.
	#[cfg(feature = "unstable_ds4")]
	#[inline(never)]
	pub fn update(&mut self, report: &DS4Report) -> Result<(), Error> {
		if !self.is_attached() {
			return Err(Error::NotPluggedIn);
		}

		self.submit(report, UPDATE_BUDGET)
	}

	// #[inline(never)]
	// pub fn update_ex(&mut self, report: &DS4ReportEx) -> Result<(), Error> {
	// 	if !self.is_attached() {
	// 		return Err(Error::NotPluggedIn);
	// 	}
	// 	unimplemented!()
	// }
}

impl<CL: Borrow<Client>> fmt::Debug for DualShock4Wired<CL> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("DualShock4Wired")
			.field("serial_no", &self.serial_no)
			.field("vendor_id", &self.id.vendor)
			.field("product_id", &self.id.product)
			.finish()
	}
}

impl<CL: Borrow<Client>> Drop for DualShock4Wired<CL> {
	#[inline]
	fn drop(&mut self) {
		let _ = self.unplug();
	}
}
