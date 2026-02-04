<p align="center">
  <img src="README-RES/icon/sendfiles.png" width="20%" />
  <br><br>
    <p align="center">
      <img src="https://img.shields.io/badge/SendFiles-1.0.0-blue?style=for-the-badge&logo=rust&logoColor=white" />
      <img src="https://img.shields.io/badge/Compatibility-Universal-green?style=for-the-badge&logo=android&logoColor=white" />
      <img src="https://img.shields.io/badge/Rust-100%25-orange?style=for-the-badge&logo=rust&logoColor=white" />
    </p>
    <p align="center">
      <b>SendFiles</b> is a desktop application designed to share any type of file with Android devices using the Quick Share protocol.<br>
      It features an optimized core for maximum compatibility and an easy-to-use interface.
    </p>
</p>

## Features

- **Universal Compatibility**: Support for almost any file extension (.sh, .apk, .apkm, .xml, .json, .webp, and more).
- **Quick Share Integration**: Fully compatible with Android's native Quick Share (Nearby Share) protocol.
- **Standalone Core**: Powered by an internalized version of `rqs_lib`, making it 100% independent.
- **Security Bypass**: Smart MIME-type mapping to ensure sensitive files like scripts are accepted by Android devices.
- **Clean Performance**: Lightweight and fast, written entirely in Rust.

## Building from Source

### 1. Install Dependencies

On Debian/Ubuntu-based systems, run:

```bash
sudo apt update && sudo apt install -y build-essential libgtk-4-dev libadwaita-1-dev gettext pkg-config curl
```

### 2. Install Rust
If you don't have Rust installed:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 3. Build Commands

### Clone the repository
```bash
git clone https://github.com/yourusername/SendFiles.git
```

### Build the project
```bash
cargo build --release
```

### Run the application
```bash
./target/release/sendfiles
```

### Insatall .deb
```bash
sudo dpkg -i /target/debian/sendfiles_1.0.0_amd64.deb
```

## Credits

Special thanks to:
- **[nozwock](https://github.com/nozwock)** for the original implementation of **rqs_lib** (`rquickshare`), which provides the backbone for the communication protocol used in this project.
- **[Martichou](https://github.com/Martichou)** for maintaining the fork of **rqs_lib** and providing the **mdns-sd** and **sys_metrics** libraries used in this project.

---
*Empowering seamless file transfers between Linux and Android.*
