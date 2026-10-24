package dev.tjommie.app;

import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;
import com.google.mlkit.vision.barcode.common.Barcode;
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions;
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning;

/**
 * Reads a QR code with Google's code scanner, which runs in Google Play services and needs no camera permission of ours.
 * A phone without it rejects with "unavailable", and the caller offers pasting the code instead. What is read is handed to
 * the caller and never logged.
 */
@CapacitorPlugin(name = "TjommiePairingScanner")
public class TjommiePairingScannerPlugin extends Plugin {
  @PluginMethod
  public void scan(PluginCall call) {
    GmsBarcodeScannerOptions options = new GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).enableAutoZoom().build();
    GmsBarcodeScanning.getClient(getActivity(), options)
      .startScan()
      .addOnSuccessListener(barcode -> {
        JSObject result = new JSObject();
        String text = barcode.getRawValue();
        result.put("text", text == null ? JSObject.NULL : text);
        call.resolve(result);
      })
      .addOnCanceledListener(() -> {
        JSObject result = new JSObject();
        result.put("text", JSObject.NULL);
        call.resolve(result);
      })
      .addOnFailureListener(error -> call.reject("The scanner is not available on this phone", "unavailable"));
  }
}
