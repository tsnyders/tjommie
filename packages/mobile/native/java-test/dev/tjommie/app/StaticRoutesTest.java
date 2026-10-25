package dev.tjommie.app;

import org.junit.Test;
import static org.junit.Assert.*;

public class StaticRoutesTest {
  @Test public void mapsExportedDocumentsOnly() {
    for (String path : new String[]{"/agents/", "/chat/", "/room/", "/settings/", "/bots/new/", "/bots/settings/"}) {
      assertEquals(path + "index.html", StaticRoutes.document(path));
    }
  }
  @Test public void leavesAssetsRootAndUnknownPathsAlone() {
    for (String path : new String[]{"/", "/_next/static/chunk.js", "/settings/index.html", "/missing/", "/../settings/"}) {
      assertNull(StaticRoutes.document(path));
    }
    assertNull(StaticRoutes.document(null));
  }
}
