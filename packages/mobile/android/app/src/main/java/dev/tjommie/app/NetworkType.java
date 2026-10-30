package dev.tjommie.app;

final class NetworkType {
  private NetworkType() {}

  static String from(boolean connected, boolean wifi, boolean cellular) {
    if (!connected) return "none";
    if (wifi) return "wifi";
    if (cellular) return "cellular";
    return "other";
  }
}
