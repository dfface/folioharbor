import { expect, test } from "@playwright/test";

import { createCollaborativePair, libraries } from "./support";

test("administrator bootstrap unlocks registration and preserves both personal libraries", async () => {
  const pair = await createCollaborativePair();
  try {
    expect(pair.aliceLibrary.role).toBe("owner");
    expect(pair.aliceLibrary.capabilities.can_invite_members).toBe(true);
    expect(pair.bobPersonalLibrary.role).toBe("owner");

    const bobLibraries = await libraries(pair.bob);
    expect(bobLibraries).toHaveLength(2);
    expect(bobLibraries).toEqual(expect.arrayContaining([
      expect.objectContaining({
        library_id: pair.bobPersonalLibrary.library_id,
        role: "owner",
      }),
      expect.objectContaining({
        library_id: pair.aliceLibrary.library_id,
        role: "reader",
      }),
    ]));
  } finally {
    await Promise.all([pair.alice.api.dispose(), pair.bob.api.dispose()]);
  }
});
