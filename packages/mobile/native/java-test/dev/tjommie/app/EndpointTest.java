package dev.tjommie.app;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;

import org.junit.Test;

public class EndpointTest {
  @Test
  public void acceptsLanLoopbackAndEmulatorHosts() {
    assertEquals("192.168.1.20", Endpoint.host(" 192.168.1.20 "));
    assertEquals("10.0.2.2", Endpoint.host("10.0.2.2"));
    assertEquals("127.0.0.1", Endpoint.host("127.0.0.1"));
    assertEquals("localhost", Endpoint.host("localhost"));
    assertEquals("fe80::1", Endpoint.host("[fe80::1]"));
    assertEquals("home-pc.local", Endpoint.host("home-pc.local"));
  }

  @Test
  public void refusesEmptyWildcardMulticastAndBroadcast() {
    for (String bad : new String[] { null, "", "   ", "0.0.0.0", "::", "[::]", "255.255.255.255", "224.0.0.1", "ff02::1", "bad host", "-x", "a..b!" }) {
      assertThrows(bad, IllegalArgumentException.class, () -> Endpoint.host(bad));
    }
  }

  @Test
  public void boundsThePortAndTheTimeout() {
    assertEquals(1, Endpoint.port(1));
    assertEquals(65535, Endpoint.port(65535));
    assertThrows(IllegalArgumentException.class, () -> Endpoint.port(0));
    assertThrows(IllegalArgumentException.class, () -> Endpoint.port(65536));
    assertEquals(1000, Endpoint.timeout(null));
    assertEquals(200, Endpoint.timeout(1));
    assertEquals(10_000, Endpoint.timeout(99_999));
    assertEquals(1500, Endpoint.timeout(1500));
  }
}
