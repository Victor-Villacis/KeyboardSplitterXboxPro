use crate::*;
use std::borrow::Borrow;
use std::{fmt, mem, ptr};
#[cfg(feature = "unstable_ds4")]
use std::{thread, time};
#[cfg(feature = "unstable_ds4")]
use winapi::shared::winerror;

/// How long [`DualShock4Wired::wait_ready`] keeps offering a neutral report
/// before it stops waiting for the HID stack to start polling this target.
///
/// The measured window on ViGEmBus 1.21.442.0 is 1-3ms, so this is ~80x
/// headroom. It is deliberately not "generous": priming is best-effort (see
/// [`DualShock4Wired::wait_ready`]), and a budget large enough to cover a
/// first-ever DS4 plug -- where Windows still has to build the HID stack --
/// would stall a multi-pad startup for seconds to buy nothing.
#[cfg(feature = "unstable_ds4")]
const READY_BUDGET: time::Duration = time::Duration::from_millis(250);

/// Gap between priming attempts. Off the input path, so a real sleep is right;
/// the value is nominal, as Windows rounds any sub-millisecond sleep up to the
/// current timer resolution (measured here: a 1ms sleep costs ~1.5ms).
#[cfg(feature = "unstable_ds4")]
const READY_GAP: time::Duration = time::Duration::from_millis(1);

/// How long [`DualShock4Wired::update`] retries a report the driver currently
/// has nowhere to put. Short: `wait_ready` already absorbed the startup window,
/// so this only covers a transient, and it sits in the caller's input path.
///
/// Retries here spin on [`thread::yield_now`] rather than sleeping. A sleep
/// cannot express this budget: `thread::sleep` on Windows rounds up to the
/// process timer resolution, so a nominal 250us gap measured 512us on the
/// cabinet and would cost up to 15.6ms on a machine that has not raised the
/// resolution -- 18x the whole ksx input-latency budget, for a wait that is
/// normally microseconds (`yield_now` measured 275ns).
#[cfg(feature = "unstable_ds4")]
const UPDATE_BUDGET: time::Duration = time::Duration::from_millis(2);

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
        DualShock4Wired {
            client,
            event,
            serial_no: 0,
            id,
        }
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
    ///
    /// For a DS4 target this also *primes* the driver: see the comment in the
    /// body. Priming is best-effort and never turns a successful plug into an
    /// error -- if it does not land, the first [`update`](Self::update) says so.
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
        // by dequeuing a pending interrupt-IN request and bails out with that
        // queue's status if there is none --
        //
        //     status = WdfIoQueueRetrieveNextRequest(this->_PendingUsbInRequests, &usbRequest);
        //     if (!NT_SUCCESS(status)) return status;
        //
        // -- STATUS_NO_MORE_ENTRIES, which surfaces to user mode as
        // ERROR_NO_MORE_ITEMS (259). Requests only land in that queue once the
        // HID stack above the PDO starts polling it, which lags this IOCTL by
        // 1-3ms. Absorb that window here so the caller's first real update is
        // not dropped on the floor.
        //
        // Note the queue is fed by hidclass itself, not by an application: with
        // no reader open at all, 50/50 submits are accepted (measured). The only
        // window where it is empty is this one, right after plug -- which is
        // exactly where a plug/wait_ready/update sequence used to land.
        //
        // X360 needs none of this: xusb22 keeps the interrupt-IN queue populated
        // from the moment the device starts, which is why the identically shaped
        // `Xbox360Wired::update` has never been seen to fail this way.
        #[cfg(feature = "unstable_ds4")]
        let _ = self.submit(&DS4Report::default(), READY_BUDGET, Some(READY_GAP));

        Ok(())
    }

    /// Submits one report exactly once, with no retry.
    ///
    /// This is upstream `vigem_target_ds4_update`'s IOCTL without upstream's
    /// error swallowing, and it is the honest view of what the driver did:
    ///
    /// * `Ok(())` -- the report was copied into the PDO's cached report and
    ///   handed to a waiting interrupt-IN request.
    /// * `Err(Error::TargetNotReady)` -- `STATUS_NO_MORE_ENTRIES`: no request
    ///   was pending, so the report was **dropped**. `SubmitReportImpl` returns
    ///   before it copies anything, so the state change is simply lost and the
    ///   next flush of the pending queue (every `DS4_QUEUE_FLUSH_PERIOD` = 5ms,
    ///   `PendingUsbRequestsTimerFunc`) re-sends the *previous* report.
    ///
    /// Upstream hides that distinction: `vigem_target_ds4_update` ignores every
    /// `GetOverlappedResult` failure except `ERROR_ACCESS_DENIED` and returns
    /// `VIGEM_ERROR_NONE` regardless. That is harmless for a client that streams
    /// reports on a timer (DS4Windows) and wrong for one that submits only on
    /// change -- the dropped edge never comes back. Prefer
    /// [`update`](Self::update), which retries; use this to observe the driver.
    #[cfg(feature = "unstable_ds4")]
    #[inline(never)]
    pub fn try_update(&mut self, report: &DS4Report) -> Result<(), Error> {
        if !self.is_attached() {
            return Err(Error::NotPluggedIn);
        }

        self.submit_once(report).map_err(Self::submit_error)
    }

    /// One `IOCTL_DS4_SUBMIT_REPORT`, raw win32 error preserved so the retry
    /// loop can tell "nothing is polling *yet*" (retry) from "this serial is
    /// gone" (do not).
    #[cfg(feature = "unstable_ds4")]
    fn submit_once(&mut self, report: &DS4Report) -> Result<(), u32> {
        unsafe {
            let mut dsr = bus::DS4SubmitReport::new(self.serial_no, *report);
            let device = self.client.borrow().device;
            dsr.ioctl(device, self.event.handle)
        }
    }

    #[cfg(feature = "unstable_ds4")]
    fn submit_error(err: u32) -> Error {
        match err {
            // STATUS_NO_MORE_ENTRIES: no interrupt-IN request was pending, so
            // the report was dropped without being cached.
            winerror::ERROR_NO_MORE_ITEMS => Error::TargetNotReady,
            // STATUS_DEVICE_DOES_NOT_EXIST: Bus_Ds4SubmitReportHandler could not
            // find this serial (sys/Queue.cpp). Matches `Xbox360Wired::update`.
            winerror::ERROR_DEV_NOT_EXIST => Error::TargetNotReady,
            err => Error::WinError(err),
        }
    }

    /// Submits one report, retrying while the driver has nowhere to put it.
    ///
    /// `gap` is the pause between attempts: `Some(_)` sleeps (startup paths),
    /// `None` spins on [`thread::yield_now`] (the caller's input path, where a
    /// Windows sleep cannot express a sub-millisecond gap -- see
    /// [`UPDATE_BUDGET`]).
    #[cfg(feature = "unstable_ds4")]
    fn submit(
        &mut self,
        report: &DS4Report,
        budget: time::Duration,
        gap: Option<time::Duration>,
    ) -> Result<(), Error> {
        let start = time::Instant::now();
        loop {
            match self.submit_once(report) {
                Ok(()) => return Ok(()),
                // Nothing is polling the target yet (or right at this instant).
                Err(winerror::ERROR_NO_MORE_ITEMS) => {
                    if start.elapsed() >= budget {
                        return Err(Error::TargetNotReady);
                    }
                    match gap {
                        Some(gap) => thread::sleep(gap),
                        None => thread::yield_now(),
                    }
                }
                Err(err) => return Err(Self::submit_error(err)),
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

        self.submit(report, UPDATE_BUDGET, None)
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
