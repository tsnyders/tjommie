package dev.tjommie.app;

import java.net.InetAddress;
import java.net.UnknownHostException;

/** Which hosts and ports the socket plugin will open. Pure Java (a literal address is checked without a lookup), so it is unit tested on the JVM. */
final class Endpoint {
  private Endpoint() {}

  /** The host trimmed, or an IllegalArgumentException saying why it is refused: empty, a wildcard, a multicast or broadcast address. Loopback and private addresses are allowed (the emulator's host is 10.0.2.2, and tests use localhost). */
  static String host(String raw) {
    String host = raw == null ? "" : raw.trim();
    if (host.isEmpty() || host.length() > 253) throw new IllegalArgumentException("Host is empty or too long");
    if (host.startsWith("[") && host.endsWith("]")) host = host.substring(1, host.length() - 1);
    if (host.equals("0.0.0.0") || host.equals("::") || host.equals("0:0:0:0:0:0:0:0")) throw new IllegalArgumentException("A wildcard address is not a host");
    if (host.equals("255.255.255.255")) throw new IllegalArgumentException("A broadcast address is not a host");
    if (host.indexOf(':') >= 0 || host.matches("[0-9.]+")) {
      try {
        InetAddress address = InetAddress.getByName(host); // a literal: no lookup happens
        if (address.isMulticastAddress() || address.isAnyLocalAddress()) throw new IllegalArgumentException("Not a host address");
      } catch (UnknownHostException error) {
        throw new IllegalArgumentException("Not a valid address");
      }
    } else if (!host.matches("[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?")) {
      throw new IllegalArgumentException("Not a valid host name");
    }
    return host;
  }

  static int port(int port) {
    if (port < 1 || port > 65535) throw new IllegalArgumentException("Port out of range");
    return port;
  }

  /** A connect timeout in milliseconds: the default 1000, kept within 200 to 10000. */
  static int timeout(Integer requested) {
    if (requested == null) return 1000;
    return Math.max(200, Math.min(10_000, requested));
  }
}
