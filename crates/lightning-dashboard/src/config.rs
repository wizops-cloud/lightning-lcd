//! Configuration du tableau de bord (section `[dashboard]` du fichier TOML).

use serde::{Deserialize, Serialize};

/// Réglages du tableau de bord.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardConfig {
    /// Images par seconde envoyées à l'écran.
    pub fps: f32,
    /// Qualité JPEG (1 à 100).
    pub quality: u8,
    /// Rotation logicielle de l'image, sens horaire : 0, 90, 180 ou 270 degrés
    /// (90 et 270 : disposition portrait).
    pub rotation: u16,
    /// Luminosité de l'écran (0 à 100) appliquée au démarrage, si renseignée.
    pub brightness: Option<u8>,
    /// Panneaux affichés : `gpu`, `cpu`, `memory`, `disks`, `network`.
    pub widgets: Vec<String>,
    /// Points de montage affichés dans le panneau disques.
    pub disks: Vec<String>,
    /// Interface réseau suivie ; par défaut la somme des interfaces physiques.
    pub interface: Option<String>,
    /// Nombre de points conservés dans les mini-historiques.
    pub history: usize,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            fps: 2.0,
            quality: 85,
            rotation: 0,
            brightness: None,
            widgets: ["gpu", "cpu", "memory", "disks", "network"]
                .map(String::from)
                .to_vec(),
            disks: vec!["/".into(), "/home".into()],
            interface: None,
            history: 120,
        }
    }
}

impl DashboardConfig {
    /// Vrai si le panneau `name` doit être affiché.
    pub fn has_widget(&self, name: &str) -> bool {
        self.widgets.iter().any(|w| w.eq_ignore_ascii_case(name))
    }

    /// Intervalle entre deux images, borné à des valeurs raisonnables.
    pub fn frame_interval(&self) -> std::time::Duration {
        let fps = if self.fps.is_finite() { self.fps } else { 2.0 };
        std::time::Duration::from_secs_f32(1.0 / fps.clamp(0.1, 30.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_enable_every_widget() {
        let cfg = DashboardConfig::default();
        for w in ["gpu", "cpu", "memory", "disks", "network"] {
            assert!(cfg.has_widget(w));
        }
        assert!(!cfg.has_widget("weather"));
        assert_eq!(cfg.frame_interval(), std::time::Duration::from_millis(500));
    }

    #[test]
    fn frame_interval_is_clamped() {
        let cfg = DashboardConfig {
            fps: 1000.0,
            ..Default::default()
        };
        assert!(cfg.frame_interval() >= std::time::Duration::from_millis(33));
        let cfg = DashboardConfig {
            fps: f32::NAN,
            ..Default::default()
        };
        assert_eq!(cfg.frame_interval(), std::time::Duration::from_millis(500));
    }
}
