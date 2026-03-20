# Maintainer: MrDemon
pkgname=sendfiles
pkgver=0.1.0
pkgrel=1
pkgdesc="Send files to other devices on the same network"
arch=('x86_64' 'aarch64')
url="https://github.com/MrDemon/SendFiles"
license=('GPL3')
depends=('gtk4' 'libadwaita' 'gettext')
makedepends=('cargo' 'git')
source=("${pkgname}::git+file://$PWD")
sha256sums=('SKIP')

build() {
  cd "$pkgname"
  cargo build --release
}

package() {
  cd "$pkgname"
  install -Dm755 "target/release/SendFiles" "$pkgdir/usr/bin/sendfiles"
  install -Dm644 "assets/sendfiles.desktop" "$pkgdir/usr/share/applications/SendFiles.desktop"
  install -Dm644 "assets/sendfiles.png" "$pkgdir/usr/share/icons/hicolor/512x512/apps/sendfiles.png"
}
