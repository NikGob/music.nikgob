{
  description = "music.nikgob: a desktop music player";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      inherit (pkgs) lib;
      version = (lib.importTOML ./Cargo.toml).workspace.package.version;

      # Linked by the binary: the sign-in window's GTK and WebKitGTK (and the
      # file dialogs' GTK), ALSA under cpal, libwayland-client under winit,
      # OpenSSL under native-tls.
      linked = with pkgs; [
        alsa-lib
        cairo
        gdk-pixbuf
        glib
        gtk3
        libsoup_3
        openssl
        pango
        wayland
        webkitgtk_4_1
      ];
      # Opened with dlopen at run time, so nothing records them: OpenGL and
      # EGL under glutin, xkbcommon and the X11 libraries under winit. They
      # go on the binary's RUNPATH rather than LD_LIBRARY_PATH, so the
      # programs the player runs (ffmpeg, curl, xdg-open) do not inherit
      # them.
      dlopened = with pkgs; [
        libGL
        libx11
        libxcursor
        libxi
        libxkbcommon
        libxrandr
        wayland
      ];

      package = pkgs.rustPlatform.buildRustPackage {
        pname = "music.nikgob";
        inherit version;

        src = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./assets
            ./crates
            ./packaging
            ./vendor
          ];
        };

        cargoLock = {
          lockFile = ./Cargo.lock;
          # One per git repository: the egui and winit forks, fastframe.
          outputHashes = {
            "ecolor-0.36.1" = "sha256-jF+5LjNkGAvdlwstYPVMIj0KKnnxHqpbcjC2ZgsUmoo=";
            "dpi-0.1.1" = "sha256-5ioLeQqbyXFgbsE3Iik/FgtHYSIFv+35hGjP8UMQl9k=";
            "fastframe-audio-0.4.1" = "sha256-2X9gzgirQc3AwsszGQqkUbggGKEE41uW9EEChegM8VM=";
          };
        };

        cargoBuildFlags = [ "-p" "nikgob" ];
        # `cargo test --workspace` runs the tests; some of them want a
        # display, a session bus or the network, none of which the sandbox
        # has.
        doCheck = false;

        nativeBuildInputs = with pkgs; [
          cmake
          pkg-config
          wrapGAppsHook3
        ];
        buildInputs =
          linked
          ++ (with pkgs; [
            # WebKitGTK's TLS, and the schemas GTK's file chooser reads.
            glib-networking
            gsettings-desktop-schemas
          ]);

        postInstall = ''
          install -Dm644 packaging/music.nikgob.desktop $out/share/applications/music.nikgob.desktop
          for size in 64 128 256 512; do
            install -Dm644 packaging/icons/music.nikgob-$size.png \
              $out/share/icons/hicolor/''${size}x''${size}/apps/music.nikgob.png
          done
        '';

        postFixup = ''
          patchelf --add-rpath ${lib.makeLibraryPath dlopened} $out/bin/.nikgob-wrapped
        '';

        meta = {
          description = "Desktop music player: a local library with streaming services, lyrics and recommendations";
          homepage = "https://github.com/NikGob/music.nikgob";
          license = lib.licenses.unfree;
          mainProgram = "nikgob";
          platforms = [ system ];
        };
      };
    in
    {
      packages.${system}.default = package;

      devShells.${system}.default = pkgs.mkShell {
        inputsFrom = [ package ];
        packages = with pkgs; [
          cargo
          clippy
          rust-analyzer
          rustc
          rustfmt
        ];
        LD_LIBRARY_PATH = lib.makeLibraryPath dlopened;
        # What the wrapper sets for the installed binary: GTK's schemas and
        # WebKitGTK's TLS module.
        shellHook = ''
          export XDG_DATA_DIRS=${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}
          export GIO_EXTRA_MODULES=${pkgs.glib-networking}/lib/gio/modules''${GIO_EXTRA_MODULES:+:$GIO_EXTRA_MODULES}
        '';
      };
    };
}
