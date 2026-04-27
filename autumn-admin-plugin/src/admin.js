// autumn-admin-plugin client-side helpers.
// Served from /{prefix}/static/admin.js so it works under CSP `script-src 'self'`.

(function () {
  // Select-all checkbox toggles every .row-check in the current table.
  document.addEventListener("click", function (e) {
    var t = e.target;
    if (t && t.id === "select-all") {
      document.querySelectorAll(".row-check").forEach(function (c) {
        c.checked = t.checked;
      });
    }
  });

  // Bulk-action submit: confirm if the selected action is marked as
  // requiring confirmation (data-confirm="1" on the option) AND require
  // at least one selected row.
  document.addEventListener("submit", function (e) {
    var form = e.target;
    if (
      !form ||
      !form.matches ||
      !form.matches('form[action$="/actions"]')
    ) {
      return;
    }
    var checked = form.querySelectorAll(
      '.row-check:checked',
    );
    if (checked.length === 0) {
      e.preventDefault();
      window.alert("Select at least one row first.");
      return;
    }
    var sel = form.querySelector('select[name="action"]');
    if (!sel) return;
    var opt = sel.options[sel.selectedIndex];
    if (
      opt &&
      opt.dataset.confirm === "1" &&
      !window.confirm(
        "Apply '" +
          opt.text +
          "' to " +
          checked.length +
          " record(s)?",
      )
    ) {
      e.preventDefault();
    }
  });

  // Cosmetic client-side strip of blank password inputs so they aren't sent.
  // The real safety net is server-side in strip_meta_fields() using the
  // declared AdminFieldKind::Password metadata; this just avoids shipping
  // empty values over the wire.
  document.addEventListener(
    "submit",
    function (e) {
      var form = e.target;
      if (!form || !form.matches || !form.matches("form")) return;
      form
        .querySelectorAll('input[type="password"]')
        .forEach(function (i) {
          if (i.value === "") i.removeAttribute("name");
        });
    },
    true,
  );
})();
