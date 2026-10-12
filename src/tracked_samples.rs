use std::{
    sync::{Arc, Condvar, Mutex, Weak},
    time::Duration,
};
use windows::{
    core::*,
    Win32::{Foundation::*, Media::MediaFoundation::*},
};

#[derive(Default)]
pub struct SamplePool {
    state: Arc<PoolState>,
}
#[derive(Default)]
struct PoolState {
    slots: Mutex<Vec<Option<IMFSample>>>,
    ready: Condvar,
}
impl SamplePool {
    pub fn new(samples: Vec<IMFSample>) -> Self {
        Self {
            state: Arc::new(PoolState {
                slots: Mutex::new(samples.into_iter().map(Some).collect()),
                ready: Condvar::new(),
            }),
        }
    }
    pub fn try_take(&self) -> Option<SampleLease> {
        let mut slots = self.state.slots.lock().unwrap_or_else(|p| p.into_inner());
        let index = slots.iter().position(Option::is_some)?;
        Some(SampleLease {
            sample: slots[index].take(),
            state: self.state.clone(),
            index,
        })
    }
    pub fn wait_take(&self) -> Result<SampleLease> {
        let slots = self.state.slots.lock().unwrap_or_else(|p| p.into_inner());
        let (mut slots, _) = self
            .state
            .ready
            .wait_timeout_while(slots, Duration::from_secs(2), |s| {
                s.iter().all(Option::is_none)
            })
            .unwrap_or_else(|p| p.into_inner());
        let index = slots
            .iter()
            .position(Option::is_some)
            .ok_or_else(|| Error::from_hresult(E_FAIL))?;
        Ok(SampleLease {
            sample: slots[index].take(),
            state: self.state.clone(),
            index,
        })
    }
}

pub struct SampleLease {
    sample: Option<IMFSample>,
    state: Arc<PoolState>,
    pub index: usize,
}
impl SampleLease {
    pub fn sample(&self) -> &IMFSample {
        self.sample.as_ref().unwrap()
    }
    pub unsafe fn submit(mut self, writer: &IMFSinkWriter, stream: u32) -> Result<()> {
        let callback: IMFAsyncCallback = ReturnSample {
            state: Arc::downgrade(&self.state),
            index: self.index,
        }
        .into();
        let tracked: IMFTrackedSample = self.sample().cast()?;
        tracked.SetAllocator(&callback, None)?;
        let sample = self.sample.take().unwrap();
        let result = writer.WriteSample(stream, &sample);
        drop(tracked);
        drop(sample);
        result
    }
}
impl Drop for SampleLease {
    fn drop(&mut self) {
        if let Some(sample) = self.sample.take() {
            self.state.slots.lock().unwrap_or_else(|p| p.into_inner())[self.index] = Some(sample);
            self.state.ready.notify_one();
        }
    }
}
#[implement(IMFAsyncCallback)]
struct ReturnSample {
    state: Weak<PoolState>,
    index: usize,
}
impl IMFAsyncCallback_Impl for ReturnSample_Impl {
    fn GetParameters(&self, _: *mut u32, _: *mut u32) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn Invoke(&self, result: Ref<'_, IMFAsyncResult>) -> Result<()> {
        let Some(state) = self.state.upgrade() else {
            return Ok(());
        };
        let sample: IMFSample = unsafe { result.ok()?.GetObject()?.cast()? };
        state.slots.lock().unwrap_or_else(|p| p.into_inner())[self.index] = Some(sample);
        state.ready.notify_one();
        Ok(())
    }
}
