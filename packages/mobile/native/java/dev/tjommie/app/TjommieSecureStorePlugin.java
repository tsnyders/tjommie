package dev.tjommie.app;

import android.content.Context;
import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/**
 * Secrets sealed under a key that lives in the Android Keystore and cannot be exported. The sealed form is
 * base64(iv || ciphertext+tag), with the secret's name as additional authenticated data, in private preferences that
 * are left out of backups. Nothing here is logged.
 */
@CapacitorPlugin(name = "TjommieSecureStore")
public class TjommieSecureStorePlugin extends Plugin {
  private static final String KEY_ALIAS = "tjommie.securestore.v1";
  private static final String PREFS = "tjommie_secure";
  private static final int MAX_VALUE = 64 * 1024;
  private static final int IV_BYTES = 12;

  private SharedPreferences prefs() {
    return getContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
  }

  private static String name(PluginCall call) {
    String name = call.getString("name");
    if (name == null || !name.matches("[a-z0-9._-]{1,64}")) throw new IllegalArgumentException("Bad name");
    return name;
  }

  private static synchronized SecretKey key() throws Exception {
    KeyStore store = KeyStore.getInstance("AndroidKeyStore");
    store.load(null);
    if (store.containsAlias(KEY_ALIAS)) return (SecretKey) store.getKey(KEY_ALIAS, null);
    KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
    generator.init(
      new KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .setRandomizedEncryptionRequired(true)
        .build()
    );
    return generator.generateKey();
  }

  @PluginMethod
  public void seal(PluginCall call) {
    try {
      String name = name(call);
      String value = call.getString("valueBase64");
      if (value == null) throw new IllegalArgumentException("No value");
      byte[] plain = Base64.decode(value, Base64.DEFAULT);
      if (plain.length == 0 || plain.length > MAX_VALUE) throw new IllegalArgumentException("Value size out of range");
      Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
      cipher.init(Cipher.ENCRYPT_MODE, key()); // the Keystore picks a fresh random IV each time
      cipher.updateAAD(name.getBytes(StandardCharsets.UTF_8));
      byte[] sealed = cipher.doFinal(plain);
      byte[] iv = cipher.getIV();
      byte[] out = new byte[iv.length + sealed.length];
      System.arraycopy(iv, 0, out, 0, iv.length);
      System.arraycopy(sealed, 0, out, iv.length, sealed.length);
      prefs().edit().putString(name, Base64.encodeToString(out, Base64.NO_WRAP)).apply();
      call.resolve();
    } catch (IllegalArgumentException error) {
      call.reject(error.getMessage(), "invalid");
    } catch (Exception error) {
      call.reject("The secure store is not available", "unavailable");
    }
  }

  @PluginMethod
  public void read(PluginCall call) {
    try {
      String name = name(call);
      String stored = prefs().getString(name, null);
      JSObject result = new JSObject();
      if (stored == null) {
        result.put("valueBase64", JSObject.NULL);
        call.resolve(result);
        return;
      }
      byte[] in = Base64.decode(stored, Base64.DEFAULT);
      if (in.length <= IV_BYTES) {
        call.reject("The stored value is damaged", "corrupt");
        return;
      }
      Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
      cipher.init(Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(128, in, 0, IV_BYTES));
      cipher.updateAAD(name.getBytes(StandardCharsets.UTF_8));
      byte[] plain = cipher.doFinal(in, IV_BYTES, in.length - IV_BYTES);
      result.put("valueBase64", Base64.encodeToString(plain, Base64.NO_WRAP));
      call.resolve(result);
    } catch (IllegalArgumentException error) {
      call.reject(error.getMessage(), "invalid");
    } catch (javax.crypto.AEADBadTagException error) {
      call.reject("The stored value failed its check", "corrupt");
    } catch (Exception error) {
      call.reject("The secure store is not available", "unavailable");
    }
  }

  @PluginMethod
  public void has(PluginCall call) {
    try {
      JSObject result = new JSObject();
      result.put("present", prefs().contains(name(call)));
      call.resolve(result);
    } catch (IllegalArgumentException error) {
      call.reject(error.getMessage(), "invalid");
    }
  }

  @PluginMethod(returnType = PluginMethod.RETURN_PROMISE)
  public void delete(PluginCall call) {
    try {
      prefs().edit().remove(name(call)).apply();
      call.resolve();
    } catch (IllegalArgumentException error) {
      call.reject(error.getMessage(), "invalid");
    }
  }
}
