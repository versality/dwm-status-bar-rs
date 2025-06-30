let
  stable = import <nixos> {};
  unstable = import <nixos-unstable> {};
in
  stable.mkShell {
    buildInputs = [
      stable.openssl
      unstable.clippy
      unstable.pkg-config
      unstable.rustc
      unstable.cargo
      unstable.rust-analyzer
    ];

    shellHook = ''
      export pkg_config_path=${stable.openssl.dev}/lib/pkgconfig
    '';
  }

