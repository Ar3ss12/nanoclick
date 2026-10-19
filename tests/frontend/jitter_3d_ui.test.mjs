import test from "node:test";
import assert from "node:assert/strict";

test("Level 1: 3D Jitter UI dependency logic & state preservation", () => {
  // Mock DOM elements and config
  const currentConfig = {
    engine: {
      position_mode: "cursor",
      jitter_radius_px: 5,
      jitter_relative_3d: true,
    },
  };

  const jitter3dRow = {
    classList: {
      classes: new Set(),
      add(c) { this.classes.add(c); },
      remove(c) { this.classes.delete(c); },
      contains(c) { return this.classes.has(c); },
    },
  };

  const jitter3dCheck = {
    disabled: false,
    checked: true,
    removeAttribute(attr) {
      if (attr === "disabled") this.disabled = false;
    },
    setAttribute(attr, val) {
      if (attr === "disabled") this.disabled = true;
    },
  };

  function updateSubSettingsVisibility(posMode, jitterR) {
    const jitter3dActive = posMode === "cursor" && jitterR > 0;
    if (jitter3dRow) {
      if (jitter3dActive) {
        jitter3dRow.classList.remove("disabled-subgroup");
      } else {
        jitter3dRow.classList.add("disabled-subgroup");
      }
    }
    if (jitter3dCheck) {
      if (jitter3dActive) {
        jitter3dCheck.removeAttribute("disabled");
        if (currentConfig?.engine) {
          jitter3dCheck.checked = !!currentConfig.engine.jitter_relative_3d;
        }
      } else {
        jitter3dCheck.setAttribute("disabled", "true");
        if (currentConfig?.engine) {
          jitter3dCheck.checked = !!currentConfig.engine.jitter_relative_3d;
        }
      }
    }
  }

  // Initial state: cursor mode + radius 5 -> enabled & checked
  updateSubSettingsVisibility("cursor", 5);
  assert.equal(jitter3dCheck.disabled, false, "checkbox should be enabled in cursor mode with radius > 0");
  assert.equal(jitter3dCheck.checked, true, "checkbox should reflect user preference");
  assert.equal(currentConfig.engine.jitter_relative_3d, true);

  // Switch to fixed mode -> disabled, but preference preserved in config
  updateSubSettingsVisibility("fixed", 5);
  assert.equal(jitter3dCheck.disabled, true, "checkbox should be disabled in fixed mode");
  assert.equal(currentConfig.engine.jitter_relative_3d, true, "config must NOT be wiped when disabled in UI");

  // Switch radius to 0 -> disabled, preference still preserved
  updateSubSettingsVisibility("cursor", 0);
  assert.equal(jitter3dCheck.disabled, true, "checkbox should be disabled when jitter radius is 0");
  assert.equal(currentConfig.engine.jitter_relative_3d, true, "config must NOT be wiped when radius is 0");

  // Return to cursor mode + radius 5 -> re-enabled, checkbox restores checked=true
  updateSubSettingsVisibility("cursor", 5);
  assert.equal(jitter3dCheck.disabled, false, "checkbox should be re-enabled");
  assert.equal(jitter3dCheck.checked, true, "checkbox should restore previously chosen true state");
  assert.equal(currentConfig.engine.jitter_relative_3d, true);
});
