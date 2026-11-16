# Paquet Fedora de lightning-lcd : démon + CLI (lightning-lcd) et application GNOME
# (lightning-lcd-settings). Les dépendances Rust sont embarquées (Source1, produite par
# packaging/rpm/build-srpm.sh), ce qui permet une construction hors ligne (mock, COPR).

%bcond check 1

Name:           lightning-lcd
Version:        0.1.0
Release:        1%{?dist}
Summary:        Driver and daemon for the LCD screen of the MSI RTX 5090 Lightning Z

# Le projet est GPL-3.0-only, la police embarquée OFL-1.1 ; le reste vient des crates
# embarquées (résumé produit par %%{cargo_license_summary} au %%build, détail dans
# LICENSE.dependencies). À mettre à jour quand Cargo.lock change.
SourceLicense:  GPL-3.0-only
License:        GPL-3.0-only AND OFL-1.1 AND (MIT OR Apache-2.0) AND IJG AND Unicode-3.0 AND (0BSD OR MIT OR Apache-2.0) AND Apache-2.0 AND ISC AND MIT AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR MIT) AND (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) AND BSD-2-Clause AND (BSD-2-Clause OR Apache-2.0 OR MIT) AND BSD-3-Clause AND (BSD-3-Clause OR Apache-2.0) AND (BSD-3-Clause OR MIT OR Apache-2.0) AND CDLA-Permissive-2.0 AND (MIT OR Apache-2.0 OR LGPL-2.1-or-later) AND (MIT OR Apache-2.0 OR Zlib) AND (MIT OR Zlib OR Apache-2.0) AND (Unlicense OR MIT) AND Zlib AND (Zlib OR Apache-2.0 OR MIT)
URL:            https://github.com/wizops-cloud/lightning-lcd
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
# cargo vendor de l'arbre des sources (build-srpm.sh)
Source1:        %{name}-%{version}-vendor.tar.xz

ExclusiveArch:  %{rust_arches}

BuildRequires:  cargo-rpm-macros >= 26
BuildRequires:  gcc
BuildRequires:  pkgconfig(libusb-1.0)
BuildRequires:  pkgconfig(libudev)
BuildRequires:  pkgconfig(x11)
BuildRequires:  pkgconfig(x11-xcb)
BuildRequires:  pkgconfig(gtk4) >= 4.14
BuildRequires:  pkgconfig(libadwaita-1) >= 1.5
BuildRequires:  systemd-rpm-macros
BuildRequires:  desktop-file-utils
BuildRequires:  appstream
%if %{with check}
BuildRequires:  dbus-daemon
%endif

Requires:       systemd-udev
# Lecture des vidéos et des flux
Recommends:     ffmpeg-free
# Interface Wayland de la fenêtre d'aperçu (chargée à l'exécution)
Recommends:     libwayland-client
Recommends:     libxkbcommon

%description
Linux driver and daemon for the built-in LCD screen of the MSI RTX 5090
Lightning Z graphics card (USB 0db0:7881, 1280x800): PC status dashboard,
image, video, slideshow and solid color. The daemon runs as a user service
and is controlled over D-Bus.

Independent project, not affiliated with MSI or NVIDIA. Writing to the
screen's flash memory is deliberately not implemented.

%package        settings
Summary:        GNOME application to configure the LCD screen of the MSI RTX 5090 Lightning Z
Requires:       %{name} = %{version}-%{release}
Requires:       hicolor-icon-theme

%description    settings
Lightning (lightning-settings) is a GNOME application that drives the
lightning daemon over D-Bus and manages its user service: live preview,
content, slideshow folders, dashboard panels and brightness.

%prep
%autosetup -n %{name}-%{version} -a1 -p1
%cargo_prep -v vendor

%build
%cargo_build
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%{cargo_vendor_manifest}

%install
install -Dpm0755 target/rpm/lightning %{buildroot}%{_bindir}/lightning
install -Dpm0755 target/rpm/lightning-settings %{buildroot}%{_bindir}/lightning-settings
install -Dpm0644 packaging/70-msi-lightning-lcd.rules \
    %{buildroot}%{_udevrulesdir}/70-msi-lightning-lcd.rules
# L'unité livrée vise ~/.local/bin ; le paquet installe dans /usr/bin.
install -d %{buildroot}%{_userunitdir}
sed 's|^ExecStart=.*|ExecStart=%{_bindir}/lightning daemon|; /^#   /d' \
    packaging/lightning-dashboard.service > %{buildroot}%{_userunitdir}/lightning-dashboard.service
chmod 0644 %{buildroot}%{_userunitdir}/lightning-dashboard.service
install -Dpm0644 packaging/cloud.wizops.Lightning.desktop \
    %{buildroot}%{_datadir}/applications/cloud.wizops.Lightning.desktop
install -Dpm0644 packaging/cloud.wizops.Lightning.metainfo.xml \
    %{buildroot}%{_metainfodir}/cloud.wizops.Lightning.metainfo.xml
install -Dpm0644 packaging/icons/cloud.wizops.Lightning.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/cloud.wizops.Lightning.svg
install -Dpm0644 packaging/icons/cloud.wizops.Lightning-symbolic.svg \
    %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps/cloud.wizops.Lightning-symbolic.svg

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/cloud.wizops.Lightning.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/cloud.wizops.Lightning.metainfo.xml
%if %{with check}
%cargo_test
%endif

# Règle udev : prise en compte par les déclencheurs de fichiers de systemd-udev.
%post
%systemd_user_post lightning-dashboard.service

%preun
%systemd_user_preun lightning-dashboard.service

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%license crates/lightning-dashboard/assets/OFL.txt
%doc README.md docs/PROTOCOL.md packaging/config.example.toml
%{_bindir}/lightning
%{_udevrulesdir}/70-msi-lightning-lcd.rules
%{_userunitdir}/lightning-dashboard.service

%files settings
%license LICENSE
%doc README.md
%{_bindir}/lightning-settings
%{_datadir}/applications/cloud.wizops.Lightning.desktop
%{_metainfodir}/cloud.wizops.Lightning.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/cloud.wizops.Lightning.svg
%{_datadir}/icons/hicolor/symbolic/apps/cloud.wizops.Lightning-symbolic.svg

%changelog
* Mon Nov 16 2026 WizOps <contact@wizops.fr> - 0.1.0-1
- Première version empaquetée
