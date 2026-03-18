{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.oryx-jobs;
  fmt = pkgs.formats.toml { };

  # Typed options win: merge them on top of the freeform settings attrset.
  configFile = fmt.generate "oryx-jobs.toml" (
    cfg.settings
    // {
      slots = cfg.slots;
      hold_ms = cfg.holdMs;
    }
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
        the oryx overlay (add <literal>inputs.oryx.overlays.default</literal>
        to your nixpkgs overlays to make it available).
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

        To find the index for a physical key, run <literal>oryx-look</literal>
        and hold the key for one second — its LED index will appear in the cell.
      '';
    };

    holdMs = lib.mkOption {
      type = lib.types.ints.positive;
      default = 1000;
      description = ''
        Duration in milliseconds a slot key must be held to reject a prompt.
        A tap (shorter than holdMs) accepts; a hold rejects.
      '';
    };

    settings = lib.mkOption {
      type = fmt.type;
      default = { };
      description = ''
        Freeform TOML configuration merged into the generated config file.
        Use this for color configuration. The <option>slots</option> and
        <option>holdMs</option> options always take precedence over keys of
        the same name set here.

        See <literal>jobs/etc/default.toml</literal> in the oryx source for
        the full set of available options.
      '';
      example = lib.literalExpression ''
        {
          colors = {
            idle    = "#000000";
            started = "#0064FF";
            finished.default = "#B4B4B4";
            finished.matches = [
              { value = 0; color = "#00FF00"; }
              { value = 1; color = "#FF0000"; }
            ];
            stage.default = "#FFC800";
            prompt = {
              waiting = "#C800FF";
              accept  = "#00FF00";
              reject  = "#FF0000";
            };
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
