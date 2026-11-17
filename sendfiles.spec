Name:           sendfiles
Version:        0.1.0
Release:        1%{?dist}
Summary:        Send files to other devices on the same network

License:        GPL-3.0-only
URL:            https://github.com/MrDemonc/SendFiles

BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  gtk4-devel
BuildRequires:  libadwaita-devel
BuildRequires:  gettext
BuildRequires:  pkgconf-pkg-config
BuildRequires:  protobuf-compiler
Requires:       gtk4
Requires:       libadwaita
Requires:       gettext

%description
SendFiles is a desktop application designed to share any type of file
with Android devices using the Quick Share protocol.

%build
cargo build --release

%install
install -Dm755 "target/release/SendFiles" "%{buildroot}%{_bindir}/sendfiles"
install -Dm644 "assets/sendfiles.desktop" "%{buildroot}%{_datadir}/applications/SendFiles.desktop"
install -Dm644 "assets/sendfiles.png" "%{buildroot}%{_datadir}/icons/hicolor/512x512/apps/sendfiles.png"

%files
%{_bindir}/sendfiles
%{_datadir}/applications/SendFiles.desktop
%{_datadir}/icons/hicolor/512x512/apps/sendfiles.png

%changelog
* Tue Nov 17 2026 MrDemon <mrdemonlich@gmail.com> - 0.1.0-1
- Initial Fedora RPM release
