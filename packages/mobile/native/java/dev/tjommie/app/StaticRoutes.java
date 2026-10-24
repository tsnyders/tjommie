package dev.tjommie.app;

final class StaticRoutes {
  private StaticRoutes() {}

  static String document(String path) {
    if (path == null) return null;
    switch (path) {
      case "/agents/": case "/chat/": case "/room/": case "/settings/":
      case "/bots/new/": case "/bots/settings/":
        return path + "index.html";
      default: return null;
    }
  }
}
