package dev.tjommie.app;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.util.Base64;
import android.webkit.WebView;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.WebViewListener;
import com.getcapacitor.annotation.CapacitorPlugin;
import java.io.BufferedInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.ProtocolException;
import java.net.Socket;
import java.util.Map;
import java.util.Objects;
import java.util.HashSet;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Raw TCP carrying length-prefixed binary frames, for the encrypted connection to a PC. No WebSocket, no HTTP and no
 * bearer token: what travels is opaque bytes. At most 8 connections, 65,535 bytes a frame,
 * 32 MiB waiting to be written per connection (more rejects "backpressure"), a connect timeout, and
 * every connection is closed when the plugin or the app goes. Frame contents and addresses are never logged.
 */
@CapacitorPlugin(name = "TjommieSocket")
public class TjommieSocketPlugin extends Plugin {
  private static final int MAX_CONNECTIONS = 8;
  private static final long MAX_PENDING_BYTES = 32L * 1024 * 1024;

  private final ExecutorService connecting = Executors.newCachedThreadPool();
  private final Map<String, Connection> connections = new ConcurrentHashMap<>();
  private final Object lifecycle = new Object();
  private final Set<Socket> opening = new HashSet<>();
  private long pageGeneration;
  private boolean destroyed;
  private WebViewListener pageListener;
  private ConnectivityManager connectivity;
  private Network activeNetwork;
  private String activeType;
  private ConnectivityManager.NetworkCallback networkCallback;

  @Override
  public void load() {
    pageListener = new WebViewListener() {
      @Override
      public void onPageStarted(WebView web) { closePage(); }
    };
    connectivity = (ConnectivityManager) getContext().getSystemService(Context.CONNECTIVITY_SERVICE);
    activeNetwork = connectivity.getActiveNetwork();
    activeType = networkType(activeNetwork);
    networkCallback = new ConnectivityManager.NetworkCallback() {
      @Override public void onAvailable(Network network) { emitNetworkChange(); }
      @Override public void onLost(Network network) { emitNetworkChange(); }
      @Override public void onCapabilitiesChanged(Network network, NetworkCapabilities capabilities) { emitNetworkChange(); }
    };
    connectivity.registerDefaultNetworkCallback(networkCallback);
  }

  private String networkType(Network network) {
    NetworkCapabilities capabilities = network == null ? null : connectivity.getNetworkCapabilities(network);
    return NetworkType.from(capabilities != null,
      capabilities != null && capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI),
      capabilities != null && capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR));
  }

  private void emitNetworkChange() {
    getActivity().runOnUiThread(() -> {
      Network network = connectivity.getActiveNetwork();
      String type = networkType(network);
      if (destroyed || (Objects.equals(network, activeNetwork) && type.equals(activeType))) return;
      activeNetwork = network;
      activeType = type;
      JSObject event = new JSObject();
      event.put("type", type);
      notifyListeners("network", event);
    });
  }

  @Override
  protected void handleOnStart() {
    // Bridge.Builder replaces the listener list after plugin load(). Register
    // after construction, and keep exactly one registration across resumes.
    getBridge().removeWebViewListener(pageListener);
    getBridge().addWebViewListener(pageListener);
  }

  private static void closeSocket(Socket socket) {
    try { socket.close(); } catch (IOException ignored) { /* already gone */ }
  }

  // Capacitor resets its JS listeners on document navigation, but the plugin
  // survives. Retire that document's wires before a new page can reconnect.
  private void closePage() {
    synchronized (lifecycle) {
      pageGeneration++;
      for (Socket socket : opening) closeSocket(socket);
      opening.clear();
      for (Connection connection : connections.values()) connection.close("local", null);
    }
  }

  private final class Connection {
    final String id = UUID.randomUUID().toString();
    final Socket socket;
    final long generation;
    final ExecutorService writer = Executors.newSingleThreadExecutor();
    final AtomicLong pending = new AtomicLong();
    final AtomicBoolean closed = new AtomicBoolean();

    Connection(Socket socket, long generation) {
      this.socket = socket;
      this.generation = generation;
    }

    void close(String reason, String message) {
      if (!closed.compareAndSet(false, true)) return;
      connections.remove(id);
      try {
        socket.close();
      } catch (IOException ignored) {
        // already gone
      }
      writer.shutdownNow();
      JSObject event = new JSObject();
      event.put("connectionId", id);
      event.put("reason", reason);
      if (message != null) event.put("message", message);
      synchronized (lifecycle) {
        if (!destroyed && generation == pageGeneration) notifyListeners("closed", event);
      }
    }
  }

  @PluginMethod
  public void open(PluginCall call) {
    final String host;
    final int port;
    final int timeout;
    try {
      host = Endpoint.host(call.getString("host"));
      port = Endpoint.port(call.getInt("port", 0));
      timeout = Endpoint.timeout(call.getInt("timeoutMs"));
    } catch (IllegalArgumentException error) {
      call.reject(error.getMessage(), "invalid");
      return;
    }
    final long generation;
    final Socket socket = new Socket();
    synchronized (lifecycle) {
      if (destroyed || connections.size() + opening.size() >= MAX_CONNECTIONS) {
        closeSocket(socket);
        call.reject(destroyed ? "The connection is closed" : "Too many connections", destroyed ? "closed" : "limit");
        return;
      }
      generation = pageGeneration;
      opening.add(socket);
    }
    try { connecting.execute(() -> {
      try {
        socket.connect(new InetSocketAddress(host, port), timeout);
        socket.setTcpNoDelay(true);
        socket.setKeepAlive(true);
      } catch (IOException error) {
        closeSocket(socket);
        getActivity().runOnUiThread(() -> {
          synchronized (lifecycle) {
            opening.remove(socket);
            if (!destroyed && generation == pageGeneration) call.reject("Could not connect", "unreachable");
          }
        });
        return;
      }
      // Complete on the UI thread, serialized with onPageStarted. A slow open
      // from an old document can neither register nor resolve into the new one.
      getActivity().runOnUiThread(() -> {
        synchronized (lifecycle) {
          opening.remove(socket);
          if (destroyed || generation != pageGeneration) { closeSocket(socket); return; }
          final Connection connection = new Connection(socket, generation);
          connections.put(connection.id, connection);
          read(connection);
          JSObject result = new JSObject();
          result.put("connectionId", connection.id);
          call.resolve(result);
        }
      });
    }); } catch (java.util.concurrent.RejectedExecutionException error) {
      synchronized (lifecycle) { opening.remove(socket); }
      closeSocket(socket);
      synchronized (lifecycle) {
        if (!destroyed && generation == pageGeneration) call.reject("The connection is closed", "closed");
      }
    }
  }

  private void read(Connection connection) {
    connecting.execute(() -> {
      try {
        InputStream in = new BufferedInputStream(connection.socket.getInputStream(), 64 * 1024);
        byte[] frame;
        while ((frame = FrameCodec.read(in)) != null) {
          JSObject event = new JSObject();
          event.put("connectionId", connection.id);
          event.put("dataBase64", Base64.encodeToString(frame, Base64.NO_WRAP));
          synchronized (lifecycle) {
            if (destroyed || connection.closed.get() || connection.generation != pageGeneration) break;
            notifyListeners("frame", event);
          }
        }
        connection.close("remote", null);
      } catch (ProtocolException error) {
        connection.close("protocol", "Bad frame");
      } catch (IOException error) {
        connection.close(connection.closed.get() ? "local" : "error", null);
      }
    });
  }

  private void reply(Connection connection, Runnable result) {
    getActivity().runOnUiThread(() -> {
      synchronized (lifecycle) {
        if (!destroyed && connection.generation == pageGeneration) result.run();
      }
    });
  }

  @PluginMethod
  public void send(PluginCall call) {
    Connection connection = connections.get(call.getString("connectionId", ""));
    if (connection == null) {
      call.reject("No such connection", "closed");
      return;
    }
    final byte[] wire;
    try {
      String data = call.getString("dataBase64");
      if (data == null) throw new IllegalArgumentException("No data");
      wire = FrameCodec.encode(Base64.decode(data, Base64.DEFAULT));
    } catch (IllegalArgumentException | ProtocolException error) {
      call.reject("Frame size out of range", "invalid");
      return;
    }
    if (connection.pending.addAndGet(wire.length) > MAX_PENDING_BYTES) {
      connection.pending.addAndGet(-wire.length);
      call.reject("Too much is waiting to be sent", "backpressure");
      return;
    }
    try {
      connection.writer.execute(() -> {
        try {
          OutputStream out = connection.socket.getOutputStream();
          out.write(wire);
          out.flush();
          reply(connection, () -> call.resolve());
        } catch (IOException error) {
          reply(connection, () -> call.reject("The connection failed", "closed"));
          connection.close("error", null);
        } finally {
          connection.pending.addAndGet(-wire.length);
        }
      });
    } catch (java.util.concurrent.RejectedExecutionException error) {
      connection.pending.addAndGet(-wire.length);
      call.reject("No such connection", "closed");
    }
  }

  @PluginMethod
  public void close(PluginCall call) {
    Connection connection = connections.get(call.getString("connectionId", ""));
    if (connection != null) connection.close("local", null);
    call.resolve();
  }

  @Override
  protected void handleOnDestroy() {
    synchronized (lifecycle) { destroyed = true; }
    if (networkCallback != null) connectivity.unregisterNetworkCallback(networkCallback);
    if (pageListener != null) getBridge().removeWebViewListener(pageListener);
    closePage();
    connecting.shutdownNow();
  }
}
