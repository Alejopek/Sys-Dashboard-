# Maintainer: alejopek alejopek62@gmail.com
pkgname=sys-dashboard
pkgver=1.1.0
pkgrel=1
pkgdesc="Monitor de sistema de escritorio nativo en Rust"
arch=('x86_64' 'aarch64')
url="https://github.com/Alejopek/Sys-Dashboard-"
license=('ISC')
depends=('gcc-libs' 'glib2' 'libglvnd' 'libx11' 'libxcb' 'wayland')
makedepends=('cargo')
source=("${pkgname}-${pkgver}.tar.gz::https://github.com/Alejopek/Sys-Dashboard-/archive/refs/tags/v${pkgver}.tar.gz")
sha256sums=('SKIP')

build() {
  cd "Sys-Dashboard--v${pkgver}"
  cargo build --release --locked
}

package() {
  cd "Sys-Dashboard--v${pkgver}"
  install -Dm755 target/release/sys-dashboard "$pkgdir/usr/bin/sys-dashboard"
  install -Dm644 sys-dashboard.desktop "$pkgdir/usr/share/applications/sys-dashboard.desktop"
}
