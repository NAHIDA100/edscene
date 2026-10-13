use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, ERROR_ALREADY_EXISTS, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject};

pub struct Instance {
    mutex: HANDLE,
    show: HANDLE,
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.show);
            let _ = CloseHandle(self.mutex);
        }
    }
}

impl Instance {
    pub fn acquire(hidden: bool) -> Result<Option<Self>, String> {
        Self::acquire_named("Local\\edscene-settings", hidden)
    }

    fn acquire_named(name: &str, hidden: bool) -> Result<Option<Self>, String> {
        let event_name: Vec<u16> = format!("{name}-show\0").encode_utf16().collect();
        let mutex_name: Vec<u16> = format!("{name}\0").encode_utf16().collect();
        unsafe {
            let show = CreateEventW(None, false, false, PCWSTR(event_name.as_ptr()))
                .map_err(|e| e.to_string())?;
            let mutex = match CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) {
                Ok(mutex) => mutex,
                Err(e) => {
                    let _ = CloseHandle(show);
                    return Err(e.to_string());
                }
            };
            let existing = GetLastError() == ERROR_ALREADY_EXISTS;
            let instance = Self { mutex, show };
            if existing {
                if !hidden {
                    SetEvent(show).map_err(|e| e.to_string())?;
                }
                return Ok(None);
            }
            Ok(Some(instance))
        }
    }

    pub fn show_requested(&self) -> bool {
        unsafe { WaitForSingleObject(self.show, 0) == WAIT_OBJECT_0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_tray_start_is_silent_and_visible_start_wakes_the_owner() {
        let name = format!("Local\\edscene-instance-test-{}", std::process::id());
        let owner = Instance::acquire_named(&name, true).unwrap().unwrap();
        assert!(Instance::acquire_named(&name, true).unwrap().is_none());
        assert!(!owner.show_requested());
        assert!(Instance::acquire_named(&name, false).unwrap().is_none());
        assert!(owner.show_requested());
        assert!(!owner.show_requested());
        drop(owner);
        assert!(Instance::acquire_named(&name, true).unwrap().is_some());
    }
}
