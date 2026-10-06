//! On Windows nothing can reach a device until "Apple Mobile Device Support" is installed: the
//! USB driver plus the service that plays usbmuxd on 127.0.0.1:27015. It normally arrives with
//! iTunes. Apple's license doesn't let anyone else ship it, so it is fetched from Apple instead:
//! download the iTunes installer, take that one package out of it and install only that.

use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use serde::Serialize;
use tauri::State;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum DriverState {
    /// Not Windows, where usbmuxd is part of the system.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
    Missing,
    /// Installed, but its service isn't answering.
    Stopped,
    /// The Microsoft Store apps bring their own copy, which only runs while they are open.
    StoreApp,
}

#[derive(Serialize, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Stage {
    #[default]
    Idle,
    Downloading,
    Unpacking,
    Verifying,
    Installing,
    Starting,
}

/// Where a running setup has got to, for the frontend to poll.
#[derive(Default)]
pub struct DriverSetup {
    stage: Mutex<Stage>,
    received: AtomicU64,
    total: AtomicU64,
    cancel: Mutex<Option<CancellationToken>>,
}

impl DriverSetup {
    fn enter(&self, stage: Stage) {
        *self.stage.lock().unwrap() = stage;
    }
}

#[derive(Serialize)]
pub struct DriverProgress {
    stage: Stage,
    received: u64,
    total: u64,
}

#[tauri::command]
pub fn apple_driver_state() -> DriverState {
    #[cfg(windows)]
    {
        setup::state()
    }
    #[cfg(not(windows))]
    {
        DriverState::Unsupported
    }
}

#[tauri::command]
pub fn apple_driver_progress(setup: State<'_, DriverSetup>) -> DriverProgress {
    DriverProgress {
        stage: *setup.stage.lock().unwrap(),
        received: setup.received.load(Ordering::Relaxed),
        total: setup.total.load(Ordering::Relaxed),
    }
}

/// Installs Apple Mobile Device Support, or starts its service if it is already installed.
#[tauri::command]
pub async fn install_apple_driver(setup: State<'_, DriverSetup>) -> Result<(), AppError> {
    let token = CancellationToken::new();
    {
        let mut guard = setup.cancel.lock().unwrap();
        if let Some(old) = guard.replace(token.clone()) {
            old.cancel();
        }
    }
    setup.received.store(0, Ordering::Relaxed);
    setup.total.store(0, Ordering::Relaxed);

    let result = tokio::select! {
        _ = token.cancelled() => Err(AppError::Canceled("Driver setup".into())),
        res = run(&setup) => res,
    };

    if !token.is_cancelled() {
        let mut guard = setup.cancel.lock().unwrap();
        *guard = None;
    }
    setup.enter(Stage::Idle);

    result
}

#[tauri::command]
pub async fn cancel_apple_driver(setup: State<'_, DriverSetup>) -> Result<(), AppError> {
    let mut guard = setup.cancel.lock().unwrap();
    if let Some(token) = guard.take() {
        token.cancel();
    }
    Ok(())
}

async fn run(setup: &DriverSetup) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        setup::run(setup).await
    }
    #[cfg(not(windows))]
    {
        let _ = setup;
        Err(AppError::Driver(
            "Apple's driver can't be installed here".into(),
            "it is only needed on Windows".into(),
        ))
    }
}

/// Everything here except hiding console windows is plain std, so it builds (and its tests
/// run) on any platform even though only Windows ever calls it.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod setup {
    use std::{
        ffi::OsStr,
        fs::File,
        io::{self, BufReader, Read, Seek, SeekFrom, Write},
        path::{Path, PathBuf},
        process::{Command, Output},
        sync::atomic::Ordering,
        time::Duration,
    };

    use super::{DriverSetup, DriverState, Stage};
    use crate::{
        device::get_usbmuxd,
        error::{AppError, chain},
    };

    /// Apple's own link to the current iTunes installer, the only place it publishes the driver.
    const INSTALLER_URL: &str = "https://www.apple.com/itunes/download/win64";
    const DRIVER_PACKAGE: &str = "AppleMobileDeviceSupport64.msi";
    const SERVICE_NAME: &str = "Apple Mobile Device Service";
    const SERVICE_BINARY: &str = r"Apple\Mobile Device Support\AppleMobileDeviceService.exe";
    const STORE_PACKAGES: [&str; 2] = [
        "AppleInc.AppleDevices_nzyj5cx40ttqa",
        "AppleInc.iTunes_nzyj5cx40ttqa",
    ];

    const CABINET_SIGNATURE: &[u8] = b"MSCF\0\0\0\0";
    const SERVICE_START_ATTEMPTS: u32 = 80;
    const SERVICE_START_INTERVAL: Duration = Duration::from_millis(500);

    // Windows Installer and UAC results: https://learn.microsoft.com/windows/win32/msi/error-codes
    const MSI_REBOOT_REQUIRED: i32 = 3010;
    const MSI_USER_EXIT: i32 = 1602;
    const MSI_ALREADY_RUNNING: i32 = 1618;
    const MSI_OTHER_VERSION_INSTALLED: i32 = 1638;
    const ELEVATION_DECLINED: i32 = 1223;

    // No double quotes in these: they travel as one command-line argument. The file they act on
    // comes in through the environment so that no path ever has to be quoted into a script.
    const VERIFY_SCRIPT: &str = "$s = Get-AuthenticodeSignature -LiteralPath $env:FLEKPAIR_FILE; \
        if ($s.Status -eq 'Valid' -and $s.SignerCertificate.Subject -match 'O=Apple Inc\\.') { exit 0 }; \
        [Console]::Out.Write([string]$s.Status + ', signed by ' + $s.SignerCertificate.Subject); \
        exit 3";
    // Process.Start rather than Start-Process, to tell a declined prompt (Win32 error 1223)
    // from a launch that failed for another reason, whose message is then printed. An exit
    // code that can't be read counts as success; the caller checks the outcome itself.
    const ELEVATE: &str = "$i = New-Object System.Diagnostics.ProcessStartInfo; \
        $i.FileName = $env:SystemRoot + '\\System32\\' + $env:FLEKPAIR_PROGRAM; \
        $i.Arguments = $env:FLEKPAIR_BEFORE + [char]34 + $env:FLEKPAIR_QUOTED + [char]34 + $env:FLEKPAIR_AFTER; \
        $i.Verb = 'runas'; $i.UseShellExecute = $true; $i.WindowStyle = 'Hidden'; \
        try { $p = [System.Diagnostics.Process]::Start($i) } \
        catch { $e = $_.Exception; while ($e.InnerException) { $e = $e.InnerException }; \
        if ($e.NativeErrorCode -eq 1223) { exit 1223 }; [Console]::Out.Write($e.Message); exit 1 }; \
        $p.WaitForExit(); $c = 0; try { $c = $p.ExitCode } catch { }; exit $c";

    pub fn state() -> DriverState {
        if service_installed() {
            DriverState::Stopped
        } else if store_app_installed() {
            DriverState::StoreApp
        } else {
            DriverState::Missing
        }
    }

    pub async fn run(setup: &DriverSetup) -> Result<(), AppError> {
        if service_installed() {
            setup.enter(Stage::Starting);
            blocking(start_service).await?;
            return wait_for_service().await;
        }

        let folder = Scratch::new()?;
        let installer = folder.0.join("iTunes64Setup.exe");
        let package = folder.0.join(DRIVER_PACKAGE);

        setup.enter(Stage::Downloading);
        download(&installer, setup).await?;

        setup.enter(Stage::Unpacking);
        {
            let (installer, package) = (installer.clone(), package.clone());
            blocking(move || extract_driver(&installer, &package)).await?;
        }
        let _ = std::fs::remove_file(&installer);

        // Held from the check to the end of the install so the checked file is the installed one.
        let _unchanged = open_denying_writes(&package).map_err(|e| {
            AppError::Driver(
                "Failed to open Apple's driver package".into(),
                e.to_string(),
            )
        })?;

        setup.enter(Stage::Verifying);
        {
            let package = package.clone();
            blocking(move || verify_signature(&package)).await?;
        }

        setup.enter(Stage::Installing);
        blocking(move || install_package(&package)).await?;

        setup.enter(Stage::Starting);
        wait_for_service().await
    }

    fn service_installed() -> bool {
        ["CommonProgramW6432", "CommonProgramFiles"]
            .into_iter()
            .filter_map(std::env::var_os)
            .any(|common| Path::new(&common).join(SERVICE_BINARY).exists())
    }

    fn store_app_installed() -> bool {
        std::env::var_os("LOCALAPPDATA").is_some_and(|local| {
            let packages = Path::new(&local).join("Packages");
            STORE_PACKAGES
                .iter()
                .any(|package| packages.join(package).exists())
        })
    }

    /// A folder for the download that is removed again however the setup ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Result<Self, AppError> {
            let path = std::env::temp_dir().join("FlekPair-apple-driver");
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).map_err(|e| {
                AppError::Filesystem(
                    "Failed to create a folder for the download".into(),
                    format!("{}: {e}", path.display()),
                )
            })?;
            Ok(Self(path))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(super) async fn download(destination: &Path, setup: &DriverSetup) -> Result<(), AppError> {
        let failed = |e: &dyn std::error::Error| {
            AppError::Driver("Download from Apple failed".into(), chain(e))
        };

        let client = reqwest::Client::builder()
            .user_agent(concat!("FlekPair/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| failed(&e))?;

        let mut response = client
            .get(INSTALLER_URL)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|e| failed(&e))?;

        // Whatever the link redirects to is going to be installed, so it has to still be Apple.
        let host = response.url().host_str().unwrap_or_default();
        if host != "apple.com" && !host.ends_with(".apple.com") {
            return Err(AppError::Driver(
                "Download from Apple failed".into(),
                format!("the download was redirected to {host}"),
            ));
        }

        setup
            .total
            .store(response.content_length().unwrap_or(0), Ordering::Relaxed);

        let mut file = File::create(destination).map_err(|e| {
            AppError::Filesystem("Failed to save the download".into(), e.to_string())
        })?;
        while let Some(chunk) = response.chunk().await.map_err(|e| failed(&e))? {
            file.write_all(&chunk).map_err(|e| {
                AppError::Filesystem("Failed to save the download".into(), e.to_string())
            })?;
            setup
                .received
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Apple's installer is an executable wrapped around a cabinet. Finds the cabinet and
    /// copies the driver package out of it.
    pub(super) fn extract_driver(installer: &Path, destination: &Path) -> Result<(), AppError> {
        let failed = |e: io::Error| {
            AppError::Driver("Failed to unpack Apple's installer".into(), e.to_string())
        };

        let mut file = File::open(installer).map_err(failed)?;
        let mut from = 0;
        while let Some(offset) = find_signature(&mut file, from).map_err(failed)? {
            from = offset + 1;

            let window = Window::new(file.try_clone().map_err(failed)?, offset).map_err(failed)?;
            // Bytes that only look like a cabinet don't parse as one.
            let Ok(mut cabinet) = cab::Cabinet::new(BufReader::new(window)) else {
                continue;
            };
            if cabinet.get_file_entry(DRIVER_PACKAGE).is_none() {
                continue;
            }

            let mut package = cabinet.read_file(DRIVER_PACKAGE).map_err(failed)?;
            let mut output = File::create(destination).map_err(failed)?;
            io::copy(&mut package, &mut output).map_err(failed)?;
            return Ok(());
        }

        Err(AppError::Driver(
            "Failed to unpack Apple's installer".into(),
            format!("it doesn't contain {DRIVER_PACKAGE}"),
        ))
    }

    /// Position of the next cabinet signature at or after `from`.
    fn find_signature(file: &mut File, from: u64) -> io::Result<Option<u64>> {
        const CHUNK: usize = 1 << 20;
        let overlap = CABINET_SIGNATURE.len() - 1;

        let mut position = file.seek(SeekFrom::Start(from))?;
        let mut buffer = vec![0; CHUNK + overlap];
        let mut carried = 0;
        loop {
            let read = file.read(&mut buffer[carried..])?;
            if read == 0 {
                return Ok(None);
            }
            let filled = carried + read;

            if let Some(index) = buffer[..filled]
                .windows(CABINET_SIGNATURE.len())
                .position(|window| window == CABINET_SIGNATURE)
            {
                return Ok(Some(position + index as u64));
            }

            // Keep the tail in case the signature straddles two reads.
            carried = filled.min(overlap);
            buffer.copy_within(filled - carried..filled, 0);
            position += (filled - carried) as u64;
        }
    }

    /// The part of a file from `start` on, addressed as if it began there.
    struct Window {
        file: File,
        start: u64,
    }

    impl Window {
        fn new(mut file: File, start: u64) -> io::Result<Self> {
            file.seek(SeekFrom::Start(start))?;
            Ok(Self { file, start })
        }
    }

    impl Read for Window {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.file.read(buffer)
        }
    }

    impl Seek for Window {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            let absolute = match position {
                SeekFrom::Start(offset) => self.file.seek(SeekFrom::Start(self.start + offset))?,
                relative => self.file.seek(relative)?,
            };
            absolute
                .checked_sub(self.start)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))
        }
    }

    /// Opens the file so that nothing can replace or rewrite it while the handle is held.
    fn open_denying_writes(path: &Path) -> io::Result<File> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 1;
            options.share_mode(FILE_SHARE_READ);
        }
        options.open(path)
    }

    /// Runs a script in Windows PowerShell without a console window flashing up.
    fn powershell(script: &str, variables: &[(&str, &OsStr)]) -> io::Result<Output> {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let mut command =
            Command::new(Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"));
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .envs(variables.iter().copied());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command.output()
    }

    /// Runs a Windows system program as administrator, with one argument that needs quoting,
    /// and waits for it. `Ok(None)` means the permission prompt was declined.
    fn elevated(
        program: &str,
        before: &str,
        quoted: &OsStr,
        after: &str,
    ) -> Result<Option<i32>, String> {
        let output = powershell(
            ELEVATE,
            &[
                ("FLEKPAIR_PROGRAM", program.as_ref()),
                ("FLEKPAIR_BEFORE", before.as_ref()),
                ("FLEKPAIR_QUOTED", quoted),
                ("FLEKPAIR_AFTER", after.as_ref()),
            ],
        )
        .map_err(|e| e.to_string())?;

        let said = String::from_utf8_lossy(&output.stdout).trim().to_string();
        match output.status.code() {
            Some(ELEVATION_DECLINED) => Ok(None),
            _ if !said.is_empty() => Err(said),
            Some(code) => Ok(Some(code)),
            None => Err("it was stopped before finishing".into()),
        }
    }

    /// The package runs with administrator rights, so Windows has to vouch that Apple signed it.
    fn verify_signature(package: &Path) -> Result<(), AppError> {
        let output =
            powershell(VERIFY_SCRIPT, &[("FLEKPAIR_FILE", package.as_os_str())]).map_err(|e| {
                AppError::Driver("Failed to check Apple's signature".into(), e.to_string())
            })?;

        if output.status.success() {
            Ok(())
        } else {
            Err(AppError::Driver(
                "The downloaded driver isn't signed by Apple".into(),
                String::from_utf8_lossy(&output.stdout).trim().to_string(),
            ))
        }
    }

    fn install_package(package: &Path) -> Result<(), AppError> {
        let result = elevated("msiexec.exe", "/i ", package.as_os_str(), " /qn /norestart")
            .map_err(|e| AppError::Driver("Failed to start Apple's installer".into(), e))?;

        match result {
            Some(0 | MSI_REBOOT_REQUIRED) => Ok(()),
            None | Some(MSI_USER_EXIT) => Err(AppError::Canceled("Driver installation".into())),
            // Installed after all, just not where it was looked for.
            Some(MSI_OTHER_VERSION_INSTALLED) => start_service(),
            Some(MSI_ALREADY_RUNNING) => Err(AppError::Driver(
                "Apple's installer couldn't run".into(),
                "another installation is in progress; let it finish and try again".into(),
            )),
            Some(code) => Err(AppError::Driver(
                "Apple's installer failed".into(),
                format!("Windows Installer error {code}"),
            )),
        }
    }

    fn start_service() -> Result<(), AppError> {
        let result = elevated("net.exe", "start ", SERVICE_NAME.as_ref(), "")
            .map_err(|e| AppError::Driver("Failed to start Apple's service".into(), e))?;

        match result {
            None => Err(AppError::Canceled("Starting Apple's service".into())),
            // Whether it worked shows in whether the service answers.
            Some(_) => Ok(()),
        }
    }

    async fn wait_for_service() -> Result<(), AppError> {
        for _ in 0..SERVICE_START_ATTEMPTS {
            if get_usbmuxd().await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(SERVICE_START_INTERVAL).await;
        }

        Err(AppError::Driver(
            "Apple's service isn't answering".into(),
            "restart the computer and open FlekPair again".into(),
        ))
    }

    async fn blocking<T: Send + 'static>(
        work: impl FnOnce() -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, AppError> {
        tauri::async_runtime::spawn_blocking(work)
            .await
            .map_err(|e| {
                AppError::Driver("Driver setup stopped unexpectedly".into(), e.to_string())
            })?
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{DriverSetup, setup};

    fn scratch(name: &str) -> std::path::PathBuf {
        let folder =
            std::env::temp_dir().join(format!("flekpair-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn extracts_the_driver_from_a_cabinet_inside_an_executable() {
        let folder = scratch("cabinet");
        let driver: Vec<u8> = (0..300_000u32).flat_map(|n| n.to_le_bytes()).collect();

        let mut builder = cab::CabinetBuilder::new();
        let folder_builder = builder.add_folder(cab::CompressionType::MsZip);
        folder_builder.add_file("iTunes64.msi");
        folder_builder.add_file("AppleMobileDeviceSupport64.msi");
        let mut writer = builder.build(std::io::Cursor::new(Vec::new())).unwrap();
        while let Some(mut file) = writer.next_file().unwrap() {
            if file.file_name() == "AppleMobileDeviceSupport64.msi" {
                file.write_all(&driver).unwrap();
            } else {
                file.write_all(&vec![7; 500_000]).unwrap();
            }
        }
        let cabinet = writer.finish().unwrap().into_inner();

        // Program code first, with bytes that look like a cabinet but aren't. The search resumes
        // one byte after those, and the real cabinet sits across the end of its first read.
        let first_read_ends = 4097 + (1 << 20) + 7;
        let mut installer = vec![0x90; first_read_ends - 3];
        installer[4096..4104].copy_from_slice(b"MSCF\0\0\0\0");
        installer.extend_from_slice(&cabinet);
        installer.extend_from_slice(&[0; 2048]);

        let installer_path = folder.join("installer.exe");
        let extracted_path = folder.join("driver.msi");
        std::fs::write(&installer_path, &installer).unwrap();

        setup::extract_driver(&installer_path, &extracted_path).unwrap();
        assert_eq!(std::fs::read(&extracted_path).unwrap(), driver);

        std::fs::write(&installer_path, &installer[..4200]).unwrap();
        assert!(setup::extract_driver(&installer_path, &extracted_path).is_err());

        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// Downloads Apple's real installer (about 200 MB). Run it when the driver setup stops
    /// working, to see whether Apple has changed its packaging:
    /// `cargo test -- --ignored apples_installer`
    #[test]
    #[ignore]
    fn apples_installer_still_contains_the_driver() {
        let folder = scratch("apple");
        let installer = folder.join("iTunes64Setup.exe");
        let package = folder.join("driver.msi");
        let progress = DriverSetup::default();

        tauri::async_runtime::block_on(setup::download(&installer, &progress)).unwrap();
        let downloaded = std::fs::metadata(&installer).unwrap().len();
        assert_eq!(
            progress.received.load(std::sync::atomic::Ordering::Relaxed),
            downloaded
        );
        assert_eq!(
            progress.total.load(std::sync::atomic::Ordering::Relaxed),
            downloaded
        );

        setup::extract_driver(&installer, &package).unwrap();
        let extracted = std::fs::read(&package).unwrap();
        // Windows Installer packages are OLE compound files.
        assert_eq!(
            extracted[..8],
            [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]
        );
        assert!(extracted.len() > 10 << 20, "only {} bytes", extracted.len());

        std::fs::remove_dir_all(&folder).unwrap();
    }
}
