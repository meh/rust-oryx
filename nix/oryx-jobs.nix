{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.oryx-jobs;
  fmt = pkgs.formats.toml { };

  configFile = fmt.generate "oryx-jobs.toml" (
    {
      slots = cfg.slots;
      hold_ms = cfg.holdMs;
    }
    // lib.optionalAttrs (cfg.colors != { }) { colors = cfg.colors; }
  );
in
{
  options.services.oryx-jobs = {

    enable = lib.mkEnableOption "oryx-jobs ZSA keyboard LED job service";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.oryx-jobs;
      defaultText = lib.literalExpression "pkgs.oryx-jobs";
      description = ''
        The oryx-jobs package to use. Defaults to the package provided by
        the oryx overlay (add `inputs.oryx.overlays.default` to your nixpkgs
        overlays to make it available).
      '';
    };

    slots = lib.mkOption {
      type = lib.types.nonEmptyListOf lib.types.ints.unsigned;
      example = [
        32
        68
      ];
      description = ''
        LED indices to use as job slots on the keyboard. At least one is
        required; the daemon refuses to start without a configured slot.

        To find the index for a physical key, run `oryx-look` and hold the
        key for one second — its LED index will appear in the cell.
      '';
    };

    holdMs = lib.mkOption {
      type = lib.types.ints.positive;
      default = 1000;
      description = ''
        Duration in milliseconds a slot key must be held to reject a prompt.
        A tap (shorter than `holdMs`) accepts; a hold rejects.
      '';
    };

    colors = lib.mkOption {
      type = fmt.type;
      default = { };
      description = ''
        Color and animation configuration for the `[colors]` section of the
        config file.

        Every color field accepts either a static hex string (`"#RRGGBB"`) or
        an animation attribute set:

        ```nix
        # static
        idle = "#000000";

        # breathe — brightness oscillates via sine wave (period defaults to 1500 ms)
        started = { animation = "breathe"; color = "#0064FF"; };

        # breathe with gradient — hue also shifts through the colors in sync
        started = { animation = "breathe"; colors = [ "#0064FF" "#00FF64" ]; period_ms = 2000; };

        # bounce — color sweeps back and forth through the gradient at full brightness
        stage.default = { animation = "bounce"; colors = [ "#FF8000" "#FFFF00" ]; };
        ```

        `colors` takes priority over `color` when both are present.
        `prompt.accept` and `prompt.reject` are static-only (used for the
        post-prompt flash animation).

        See `jobs/default.toml` in the oryx source for all available fields
        and their hardcoded defaults.
      '';
      example = lib.literalExpression ''
        {
          idle    = "#000000";
          started = { animation = "breathe"; color = "#0064FF"; };

          progress = {
            start = "#0064FF";
            end   = "#00FF64";
          };

          finished = {
            default = "#B4B4B4";
            matches = [
              { value = 0; color = "#00FF00"; }
              { value = 1; color = "#FF0000"; }
            ];
          };

          stage = {
            default = { animation = "bounce"; colors = [ "#FFC800" "#FF8000" ]; };
            matches = [
              { name = "compiling"; color = "#FF8000"; }
            ];
          };

          prompt = {
            waiting = { animation = "breathe"; color = "#C800FF"; period_ms = 1500; };
            accept  = "#00FF00";
            reject  = "#FF0000";
          };
        }
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = config.hardware.keyboard.zsa.enable;
        message = ''
          services.oryx-jobs requires hardware.keyboard.zsa.enable = true
          for HID device access to ZSA keyboards.
        '';
      }
    ];

    systemd.user.services.oryx-jobs = {
      description = "oryx-jobs — ZSA keyboard LED job service";
      wantedBy = [ "default.target" ];
      after = [ "dbus.socket" ];

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/oryx-jobs --config ${configFile}";
        Restart = "on-failure";
        RestartSec = "2s";

        # Minimal sandboxing — the service only needs HID devices and the
        # session DBus socket; no filesystem writes required.
        PrivateTmp = true;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        RestrictNamespaces = true;
      };
    };
  };
}
