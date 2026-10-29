(function () {
        var stored = null;
        try {
          stored = localStorage.getItem("dotlog.theme");
        } catch (e) {
          stored = null;
        }
        var theme =
          stored === "light" || stored === "dark"
            ? stored
            : window.matchMedia("(prefers-color-scheme: dark)").matches
              ? "dark"
              : "light";
        document.documentElement.dataset.theme = theme;
      })();
