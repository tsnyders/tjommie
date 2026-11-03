package dev.tjommie.app;

import java.util.Arrays;
import com.getcapacitor.PermissionState;
import org.junit.Test;
import static org.junit.Assert.*;

public class TjommieSpeechPluginTest {
  @Test public void distinguishesFirstPermissionFromDenialAndConfirmedSupport() {
    assertEquals("permission_required", TjommieSpeechPlugin.permissionProblem(PermissionState.PROMPT));
    assertEquals("permission_required", TjommieSpeechPlugin.permissionProblem(PermissionState.PROMPT_WITH_RATIONALE));
    assertEquals("permission_denied", TjommieSpeechPlugin.permissionProblem(PermissionState.DENIED));
    assertNull(TjommieSpeechPlugin.permissionProblem(PermissionState.GRANTED));
  }
  @Test public void advertisesOnlyConfirmedEnglishSupportWithoutAddingTheDesiredDefault() {
    assertTrue(TjommieSpeechPlugin.confirmedEnglishLanguages(null).isEmpty());
    assertEquals(Arrays.asList("en-GB"), TjommieSpeechPlugin.confirmedEnglishLanguages(Arrays.asList("en-GB", "af-ZA", "en-GB", null)));
    assertFalse(TjommieSpeechPlugin.supportedLanguage("en-US", TjommieSpeechPlugin.confirmedEnglishLanguages(Arrays.asList("en-GB"))));
  }
  @Test public void selectedLanguageMustBeInstalledEnglish() {
    assertTrue(TjommieSpeechPlugin.supportedLanguage("en-US", Arrays.asList("en-US", "af-ZA")));
    assertFalse(TjommieSpeechPlugin.supportedLanguage("en-GB", Arrays.asList("en-US")));
    assertFalse(TjommieSpeechPlugin.supportedLanguage("af-ZA", Arrays.asList("af-ZA")));
    assertFalse(TjommieSpeechPlugin.supportedLanguage(null, Arrays.asList("en-US")));
  }
}
