{
  description = "LEZ Bridge mini-app for Basecamp: the private Ethereum <-> LEZ bridge";

  nixConfig = {
    extra-substituters = [ "https://cache.nix.logos.co/public" ];
    extra-trusted-public-keys = [ "public:l4HrXgL4nw246+LBh2SOJyhz64BoGegOYLheT/iIAPU=" ];
  };

  inputs = {
    # The builder revision Basecamp 0.3.x locks (the same pin as the Logos Names app).
    logos-module-builder.url = "github:logos-co/logos-module-builder/de169fd45d7bdf6e28ce1f151b4c34880523ace9";
    # The core module this view is a front end for; the input name matches the dependency name
    # in metadata.json. From a copy that is not a git checkout, pass
    # `--override-input lez_bridge path:/abs/path/to/modules/lez_bridge`.
    lez_bridge.url = "path:../lez_bridge";
    lez_bridge.inputs.logos-module-builder.follows = "logos-module-builder";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosQmlModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
