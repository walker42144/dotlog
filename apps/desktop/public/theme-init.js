(function () {
        var stored = null;
        try {
          stored = localStorage.getItem("dotlog.theme");
        } catch (e) {
          stored = null;
        }
        var theme =
          ["light", "dark", "paper", "forest", "ocean", "lavender", "rose", "grid", "folio", "console", "glass", "acrylic", "obsidian", "ceramic"].includes(stored)
            ? stored
            : window.matchMedia("(prefers-color-scheme: dark)").matches
              ? "dark"
              : "light";
        document.documentElement.dataset.theme = theme;
        try { document.documentElement.dataset.motion = localStorage.getItem("dotlog.motion") === "reduced" ? "reduced" : "system"; }
        catch (e) { document.documentElement.dataset.motion = "system"; }
      })();
