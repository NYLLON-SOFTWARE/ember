import assert from "node:assert/strict"
import test from "node:test"
import { maskProductBranding } from "../branding.ts"

test("product branding normalizes while session and rich-text contracts remain visible", () => {
  assert.equal(maskProductBranding("Welcome to Campfire"), "Welcome to Ember")
  assert.equal(maskProductBranding("/assets/campfire-icon-3d9986c5.png"), maskProductBranding("/assets/ember-icon-01234567.png"))
  assert.equal(maskProductBranding("/assets/matchbox-icon-3d9986c5.png"), maskProductBranding("/assets/ember-icon-3d9986c5.png"))
  for (const contract of ["_campfire_session", "gid://campfire/User/1", "application/vnd.campfire.mention"]) {
    assert.equal(maskProductBranding(contract), contract)
  }
})
