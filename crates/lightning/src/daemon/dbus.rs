//! Adaptateur D-Bus du démon : interface `cloud.wizops.Lightning1`.
//!
//! Les méthodes transmettent une [`Request`] au moteur ; les propriétés lisent
//! l'état public. Les `PropertiesChanged` sont émis à partir des différences
//! entre deux états publics successifs.

use std::collections::HashMap;

use lightning_ipc::{BUS_NAME, Fit, INTERFACE, Mode, OBJECT_PATH, OnExit, WIDGETS};
use tracing::warn;
use zbus::blocking::Connection;
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::zvariant::Value;

use super::{Handle, PublicState, Request};
use crate::slideshow::SlideshowConfig;

/// Objet exposé sur le bus.
pub struct LightningService {
    handle: Handle,
}

impl LightningService {
    fn call(&self, request: Request) -> fdo::Result<()> {
        self.handle.call(request).map_err(fdo::Error::Failed)
    }
}

fn invalid(message: String) -> fdo::Error {
    fdo::Error::InvalidArgs(message)
}

#[zbus::interface(name = "cloud.wizops.Lightning1")]
impl LightningService {
    async fn set_mode(&self, mode: &str) -> fdo::Result<()> {
        self.call(Request::SetMode(mode.parse::<Mode>().map_err(invalid)?))
    }

    async fn show_image(&self, path: &str, fit: &str) -> fdo::Result<()> {
        self.call(Request::ShowImage {
            path: path.to_owned(),
            fit: fit.parse::<Fit>().map_err(invalid)?,
        })
    }

    async fn play_video(&self, path: &str, looped: bool) -> fdo::Result<()> {
        self.call(Request::PlayVideo {
            path: path.to_owned(),
            looped,
        })
    }

    async fn show_color(&self, rgb: &str) -> fdo::Result<()> {
        self.call(Request::ShowColor(rgb.to_owned()))
    }

    async fn restore(&self) -> fdo::Result<()> {
        self.call(Request::Restore)
    }

    async fn set_brightness(&self, level: u8) -> fdo::Result<()> {
        self.call(Request::SetBrightness(level))
    }

    async fn set_fps(&self, fps: f64) -> fdo::Result<()> {
        self.call(Request::SetFps(fps))
    }

    async fn set_rotation(&self, degrees: u16) -> fdo::Result<()> {
        self.call(Request::SetRotation(degrees))
    }

    async fn set_widgets(&self, widgets: Vec<String>) -> fdo::Result<()> {
        self.call(Request::SetWidgets(widgets))
    }

    async fn set_on_exit(&self, behaviour: &str) -> fdo::Result<()> {
        self.call(Request::SetOnExit(
            behaviour.parse::<OnExit>().map_err(invalid)?,
        ))
    }

    async fn set_http(&self, enabled: bool, addr: &str) -> fdo::Result<()> {
        self.call(Request::SetHttp {
            enabled,
            addr: addr.to_owned(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn set_slideshow(
        &self,
        folders: Vec<String>,
        interval_s: f64,
        order: &str,
        fit: &str,
        transition_ms: u32,
        overlay: bool,
        clock: bool,
    ) -> fdo::Result<()> {
        self.call(Request::SetSlideshow(SlideshowConfig {
            folders,
            interval_s,
            order: order.parse().map_err(invalid)?,
            fit: fit.parse().map_err(invalid)?,
            transition_ms,
            overlay,
            clock,
        }))
    }

    async fn slideshow_next(&self) -> fdo::Result<()> {
        self.call(Request::SlideshowNext)
    }

    async fn set_google_credentials(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> fdo::Result<()> {
        self.call(Request::SetGoogleCredentials {
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
        })
    }

    async fn google_photos_start(&self) -> fdo::Result<()> {
        self.call(Request::GoogleStart)
    }

    async fn google_photos_cancel(&self) -> fdo::Result<()> {
        self.call(Request::GoogleCancel)
    }

    async fn google_photos_clear_cache(&self) -> fdo::Result<()> {
        self.call(Request::GoogleClearCache)
    }

    async fn get_frame_jpeg(&self) -> Vec<u8> {
        self.handle.frame_jpeg()
    }

    async fn save_config(&self) -> fdo::Result<()> {
        self.call(Request::SaveConfig)
    }

    async fn reload_config(&self) -> fdo::Result<()> {
        self.call(Request::ReloadConfig)
    }

    #[zbus(property)]
    async fn lcd_state(&self) -> String {
        self.handle.state().lcd_state
    }

    #[zbus(property)]
    async fn lcd_detail(&self) -> String {
        self.handle.state().lcd_detail
    }

    #[zbus(property)]
    async fn mode(&self) -> String {
        self.handle.state().mode
    }

    #[zbus(property)]
    async fn brightness(&self) -> i32 {
        self.handle.state().brightness
    }

    #[zbus(property)]
    async fn firmware_brightness(&self) -> i32 {
        self.handle.state().firmware_brightness
    }

    #[zbus(property)]
    async fn fps(&self) -> f64 {
        self.handle.state().fps
    }

    #[zbus(property)]
    async fn rotation(&self) -> u16 {
        self.handle.state().rotation
    }

    #[zbus(property)]
    async fn widgets(&self) -> Vec<String> {
        self.handle.state().widgets
    }

    #[zbus(property)]
    async fn available_widgets(&self) -> Vec<String> {
        WIDGETS.map(String::from).to_vec()
    }

    #[zbus(property)]
    async fn on_exit(&self) -> String {
        self.handle.state().on_exit
    }

    #[zbus(property)]
    async fn http_enabled(&self) -> bool {
        self.handle.state().http_enabled
    }

    #[zbus(property)]
    async fn http_addr(&self) -> String {
        self.handle.state().http_addr
    }

    #[zbus(property)]
    async fn image_path(&self) -> String {
        self.handle.state().image_path
    }

    #[zbus(property)]
    async fn image_fit(&self) -> String {
        self.handle.state().image_fit
    }

    #[zbus(property)]
    async fn video_path(&self) -> String {
        self.handle.state().video_path
    }

    #[zbus(property)]
    async fn video_loop(&self) -> bool {
        self.handle.state().video_loop
    }

    #[zbus(property)]
    async fn color(&self) -> String {
        self.handle.state().color
    }

    #[zbus(property)]
    async fn measured_fps(&self) -> f64 {
        self.handle.state().measured_fps
    }

    #[zbus(property)]
    async fn render_ms(&self) -> f64 {
        self.handle.state().render_ms
    }

    #[zbus(property)]
    async fn frames_sent(&self) -> u64 {
        self.handle.state().frames_sent
    }

    #[zbus(property)]
    async fn slideshow_folders(&self) -> Vec<String> {
        self.handle.state().slideshow_folders
    }

    #[zbus(property)]
    async fn slideshow_interval(&self) -> f64 {
        self.handle.state().slideshow_interval
    }

    #[zbus(property)]
    async fn slideshow_order(&self) -> String {
        self.handle.state().slideshow_order
    }

    #[zbus(property)]
    async fn slideshow_fit(&self) -> String {
        self.handle.state().slideshow_fit
    }

    #[zbus(property)]
    async fn slideshow_transition_ms(&self) -> u32 {
        self.handle.state().slideshow_transition_ms
    }

    #[zbus(property)]
    async fn slideshow_overlay(&self) -> bool {
        self.handle.state().slideshow_overlay
    }

    #[zbus(property)]
    async fn slideshow_clock(&self) -> bool {
        self.handle.state().slideshow_clock
    }

    #[zbus(property)]
    async fn slideshow_count(&self) -> u32 {
        self.handle.state().slideshow_count
    }

    #[zbus(property)]
    async fn slideshow_current(&self) -> String {
        self.handle.state().slideshow_current
    }

    #[zbus(property)]
    async fn google_state(&self) -> String {
        self.handle.state().google_state
    }

    #[zbus(property)]
    async fn google_detail(&self) -> String {
        self.handle.state().google_detail
    }

    #[zbus(property)]
    async fn google_url(&self) -> String {
        self.handle.state().google_url
    }

    #[zbus(property)]
    async fn google_cached_count(&self) -> u32 {
        self.handle.state().google_cached_count
    }

    #[zbus(property)]
    async fn google_client_id(&self) -> String {
        self.handle.state().google_client_id
    }

    #[zbus(property)]
    async fn google_cache_dir(&self) -> String {
        self.handle.state().google_cache_dir
    }

    #[zbus(property)]
    async fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_owned()
    }
}

/// Publie le démon sur le bus (de session par défaut, ou à l'adresse donnée).
pub fn serve(handle: Handle, address: Option<&str>) -> zbus::Result<Connection> {
    let builder = match address {
        Some(address) => zbus::blocking::connection::Builder::address(address)?,
        None => zbus::blocking::connection::Builder::session()?,
    };
    let connection = builder
        .serve_at(OBJECT_PATH, LightningService { handle })?
        .build()?;
    // Nom demandé sans file d'attente ni remplacement : un seul démon à la fois.
    match connection.request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())? {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(connection),
        _ => Err(zbus::Error::NameTaken),
    }
}

/// Propriétés dont la valeur diffère entre deux états.
pub fn changed_properties(
    old: Option<&PublicState>,
    new: &PublicState,
) -> HashMap<&'static str, Value<'static>> {
    let mut changed = HashMap::new();
    macro_rules! diff {
        ($($field:ident => $name:literal),+ $(,)?) => {
            $(if old.is_none_or(|o| o.$field != new.$field) {
                changed.insert($name, Value::new(new.$field.clone()));
            })+
        };
    }
    diff! {
        lcd_state => "LcdState",
        lcd_detail => "LcdDetail",
        mode => "Mode",
        brightness => "Brightness",
        firmware_brightness => "FirmwareBrightness",
        fps => "Fps",
        rotation => "Rotation",
        widgets => "Widgets",
        on_exit => "OnExit",
        http_enabled => "HttpEnabled",
        http_addr => "HttpAddr",
        image_path => "ImagePath",
        image_fit => "ImageFit",
        video_path => "VideoPath",
        video_loop => "VideoLoop",
        color => "Color",
        measured_fps => "MeasuredFps",
        render_ms => "RenderMs",
        frames_sent => "FramesSent",
        slideshow_folders => "SlideshowFolders",
        slideshow_interval => "SlideshowInterval",
        slideshow_order => "SlideshowOrder",
        slideshow_fit => "SlideshowFit",
        slideshow_transition_ms => "SlideshowTransitionMs",
        slideshow_overlay => "SlideshowOverlay",
        slideshow_clock => "SlideshowClock",
        slideshow_count => "SlideshowCount",
        slideshow_current => "SlideshowCurrent",
        google_state => "GoogleState",
        google_detail => "GoogleDetail",
        google_url => "GoogleUrl",
        google_cached_count => "GoogleCachedCount",
        google_client_id => "GoogleClientId",
        google_cache_dir => "GoogleCacheDir",
    }
    changed
}

/// Observateur du moteur qui émet `PropertiesChanged` pour ce qui a changé.
pub fn notifier(connection: Connection) -> Box<dyn FnMut(&PublicState) + Send> {
    let mut previous: Option<PublicState> = None;
    Box::new(move |state| {
        let changed = changed_properties(previous.as_ref(), state);
        previous = Some(state.clone());
        if changed.is_empty() {
            return;
        }
        let body = (INTERFACE, changed, Vec::<String>::new());
        if let Err(e) = connection.emit_signal(
            None::<zbus::names::BusName<'_>>,
            OBJECT_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &body,
        ) {
            warn!("signal PropertiesChanged non émis : {e}");
        }
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::{start, wait_for, write_png};
    use super::*;
    use lightning_ipc::LightningProxyBlocking;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};

    /// Bus de session privé (`dbus-daemon`), arrêté à la destruction.
    pub struct PrivateBus {
        child: Child,
        pub address: String,
    }

    impl PrivateBus {
        /// `None` si `dbus-daemon` n'est pas installé : le test est alors sauté.
        pub fn start() -> Option<Self> {
            let mut child = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let mut address = String::new();
            BufReader::new(child.stdout.take()?)
                .read_line(&mut address)
                .ok()?;
            let address = address.trim().to_owned();
            if address.is_empty() {
                let _ = child.kill();
                return None;
            }
            Some(Self { child, address })
        }

        pub fn connect(&self) -> Connection {
            zbus::blocking::connection::Builder::address(self.address.as_str())
                .unwrap()
                .build()
                .unwrap()
        }
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn changed_properties_diff() {
        let a = PublicState {
            mode: "dashboard".into(),
            fps: 2.0,
            ..Default::default()
        };
        assert_eq!(changed_properties(None, &a).len(), 34);
        let mut b = a.clone();
        assert!(changed_properties(Some(&a), &b).is_empty());
        b.mode = "color".into();
        b.widgets = vec!["cpu".into()];
        let changed = changed_properties(Some(&a), &b);
        assert_eq!(changed.len(), 2);
        assert_eq!(changed["Mode"], Value::new("color"));
        assert!(changed.contains_key("Widgets"));
    }

    #[test]
    fn dbus_interface_on_a_private_bus() {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("dbus-daemon introuvable : test D-Bus sauté");
            return;
        };
        let run = start("dbus", |_| {}, None);
        let service = serve(run.handle.clone(), Some(&bus.address)).unwrap();
        // Le moteur tourne déjà : on branche l'émetteur en passant par un relais.
        let relay = std::sync::Arc::new(std::sync::Mutex::new(notifier(service.clone())));
        let handle = run.handle.clone();
        let stop = run.stop.clone();
        let pump = std::thread::spawn({
            let relay = relay.clone();
            move || {
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    (relay.lock().unwrap())(&handle.state());
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        });

        let client = bus.connect();
        let proxy = LightningProxyBlocking::new(&client).unwrap();
        assert_eq!(proxy.version().unwrap(), env!("CARGO_PKG_VERSION"));
        assert_eq!(proxy.mode().unwrap(), "dashboard");
        assert_eq!(proxy.lcd_state().unwrap(), "disabled");
        assert_eq!(proxy.available_widgets().unwrap(), WIDGETS);
        assert_eq!(proxy.brightness().unwrap(), -1);

        // Méthodes + propriétés mises à jour par PropertiesChanged (cache du proxy).
        proxy.show_color("0000ff").unwrap();
        wait_for("Mode=color signalé", || proxy.mode().unwrap() == "color");
        assert_eq!(proxy.color().unwrap(), "0000ff");

        proxy.set_fps(8.0).unwrap();
        proxy.set_widgets(&["network", "cpu"]).unwrap();
        proxy.set_on_exit("blank").unwrap();
        proxy.set_brightness(55).unwrap();
        wait_for("propriétés signalées", || {
            proxy.fps().unwrap() == 8.0
                && proxy.widgets().unwrap() == ["network", "cpu"]
                && proxy.on_exit().unwrap() == "blank"
                && proxy.brightness().unwrap() == 55
        });

        let img = run.dir.join("img.png");
        write_png(&img, 64, 40, [255, 255, 0], [255, 255, 0]);
        proxy.show_image(img.to_str().unwrap(), "contain").unwrap();
        wait_for("Mode=image signalé", || proxy.mode().unwrap() == "image");
        assert_eq!(proxy.image_fit().unwrap(), "contain");

        let jpeg = proxy.get_frame_jpeg().unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1280, 800));

        // Erreurs lisibles côté client.
        let err = proxy.set_mode("tv").unwrap_err().to_string();
        assert!(err.contains("tv"), "{err}");
        let err = proxy
            .show_image("/nonexistent.png", "cover")
            .unwrap_err()
            .to_string();
        assert!(err.contains("/nonexistent.png"), "{err}");
        assert!(proxy.set_rotation(90).is_err());
        assert!(proxy.set_on_exit("explode").is_err());

        // Diaporama : réglages et propriétés.
        let photos = run.dir.join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        write_png(&photos.join("p.png"), 32, 20, [5, 5, 250], [5, 5, 250]);
        proxy
            .set_slideshow(
                &[photos.to_str().unwrap()],
                5.0,
                "chronological",
                "cover",
                0,
                false,
                true,
            )
            .unwrap();
        proxy.set_mode("slideshow").unwrap();
        wait_for("diaporama signalé", || {
            proxy.mode().unwrap() == "slideshow" && proxy.slideshow_count().unwrap() == 1
        });
        assert_eq!(
            proxy.slideshow_folders().unwrap(),
            [photos.to_str().unwrap()]
        );
        assert_eq!(proxy.slideshow_interval().unwrap(), 5.0);
        assert_eq!(proxy.slideshow_order().unwrap(), "chronological");
        assert!(proxy.slideshow_clock().unwrap() && !proxy.slideshow_overlay().unwrap());
        proxy.slideshow_next().unwrap();
        assert!(
            proxy
                .set_slideshow(&[], 5.0, "alpha", "cover", 0, false, true)
                .is_err()
        );
        assert_eq!(proxy.google_state().unwrap(), "unconfigured");
        assert!(
            proxy
                .google_photos_start()
                .unwrap_err()
                .to_string()
                .contains("client_id")
        );
        proxy.google_photos_clear_cache().unwrap();
        assert_eq!(proxy.google_cached_count().unwrap(), 0);

        proxy.set_mode("dashboard").unwrap();
        proxy.save_config().unwrap();
        proxy.reload_config().unwrap();
        wait_for("Mode=dashboard signalé", || {
            proxy.mode().unwrap() == "dashboard"
        });

        // Un second démon ne peut pas prendre le même nom.
        assert!(serve(run.handle.clone(), Some(&bus.address)).is_err());

        run.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        pump.join().unwrap();
        drop(proxy);
    }
}
