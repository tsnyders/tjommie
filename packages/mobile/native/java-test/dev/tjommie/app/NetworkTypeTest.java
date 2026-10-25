package dev.tjommie.app;

import org.junit.Test;
import static org.junit.Assert.*;

public class NetworkTypeTest {
  @Test public void mapsActiveTransports() {
    assertEquals("none", NetworkType.from(false, false, false));
    assertEquals("wifi", NetworkType.from(true, true, false));
    assertEquals("cellular", NetworkType.from(true, false, true));
    assertEquals("other", NetworkType.from(true, false, false));
    assertEquals("wifi", NetworkType.from(true, true, true));
  }
}
