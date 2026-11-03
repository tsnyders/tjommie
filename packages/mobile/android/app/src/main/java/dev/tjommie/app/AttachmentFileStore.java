package dev.tjommie.app;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.function.BooleanSupplier;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HashMap;
import java.util.Map;
import java.util.UUID;

final class AttachmentFileStore {
  static final long MAX_BYTES = 200L * 1024 * 1024;
  private static final class Save {
    final File file; final FileOutputStream output; final MessageDigest hash; final long size; final String digest;
    long written; boolean verified;
    Save(File file, long size, String digest) throws IOException { this.file = file; this.size = size; this.digest = digest; output = new FileOutputStream(file); hash = sha256(); }
  }
  private final File root;
  private final Map<String, Save> saves = new HashMap<>();
  AttachmentFileStore(File root) throws IOException {
    this.root = root.getCanonicalFile(); if (!this.root.isDirectory() && !this.root.mkdirs()) throw new IOException("Download staging unavailable");
    File[] old = this.root.listFiles(); if (old != null) for (File file : old) if (file.getName().matches("[0-9a-f-]{36}\\.part") && file.getCanonicalFile().getParentFile().equals(this.root) && !file.delete()) throw new IOException("Old download cleanup failed");
  }
  static MessageDigest sha256() { try { return MessageDigest.getInstance("SHA-256"); } catch (NoSuchAlgorithmException error) { throw new IllegalStateException(error); } }
  static String hex(byte[] bytes) { StringBuilder out = new StringBuilder(); for (byte value : bytes) out.append(String.format(java.util.Locale.ROOT, "%02x", value & 255)); return out.toString(); }
  static void copy(InputStream input, OutputStream output, byte[] buffer, BooleanSupplier cancelled) throws IOException {
    for (int count; (count = input.read(buffer)) >= 0;) {
      if (cancelled.getAsBoolean()) throw new IOException("Download cancelled");
      if (count > 0) output.write(buffer, 0, count);
    }
    if (cancelled.getAsBoolean()) throw new IOException("Download cancelled");
    output.flush();
  }
  synchronized String start(long size, String digest) throws IOException {
    if (size < 0 || size > MAX_BYTES || digest == null || !digest.matches("[a-f0-9]{64}") || saves.size() >= 2) throw new IOException("Invalid download or two downloads already active");
    String id = UUID.randomUUID().toString(); File file = new File(root, id + ".part");
    if (!file.createNewFile()) throw new IOException("Download staging unavailable");
    try { saves.put(id, new Save(file, size, digest)); return id; } catch (IOException error) { file.delete(); throw error; }
  }
  synchronized void append(String id, long offset, byte[] bytes) throws IOException {
    Save save = saves.get(id);
    if (save == null || save.verified || bytes == null || bytes.length < 1 || bytes.length > AttachmentInputStream.CHUNK || offset != save.written || offset + bytes.length > save.size) throw new IOException("Invalid download chunk");
    try { save.output.write(bytes); save.hash.update(bytes); save.written += bytes.length; }
    catch (IOException error) { cancel(id); throw error; }
  }
  synchronized File verify(String id) throws IOException {
    Save save = saves.get(id); if (save == null) throw new IOException("Download unavailable");
    if (!save.verified) {
      if (save.written != save.size || !hex(save.hash.digest()).equals(save.digest)) { cancel(id); throw new IOException("Download content check failed"); }
      try { save.output.getFD().sync(); save.output.close(); save.verified = true; } catch (IOException error) { cancel(id); throw error; }
    }
    return save.file;
  }
  synchronized String digest(String id) throws IOException { Save save = saves.get(id); if (save == null) throw new IOException("Download unavailable"); return save.digest; }
  synchronized void cancel(String id) throws IOException {
    Save save = saves.get(id); if (save == null) return; try { save.output.close(); } finally { if (save.file.exists() && !save.file.delete()) throw new IOException("Download cleanup failed"); } saves.remove(id);
  }
  synchronized void clear() throws IOException { IOException failure = null; for (String id : saves.keySet().toArray(new String[0])) try { cancel(id); } catch (IOException error) { failure = error; } if (failure != null) throw failure; }
}
