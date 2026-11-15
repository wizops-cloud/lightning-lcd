//! Tableau de bord de l'état du PC : collecte des métriques ([`metrics`]) et
//! rendu d'une image 1280×800 ([`render`]), sans dépendance à l'écran.

pub mod config;
pub mod metrics;
pub mod render;

pub use config::DashboardConfig;
pub use metrics::{Collector, History, Snapshot};
pub use render::Renderer;
