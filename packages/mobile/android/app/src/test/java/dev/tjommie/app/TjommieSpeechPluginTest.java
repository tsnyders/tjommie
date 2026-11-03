package dev.tjommie.app;

import java.util.Arrays;
import org.junit.Test;
import static org.junit.Assert.*;

public class TjommieSpeechPluginTest {
  @Test public void selectedLanguageMustBeInstalledEnglish() {
    assertTrue(TjommieSpeechPlugin.supportedLanguage("en-US", Arrays.asList("en-US", "af-ZA")));
    assertFalse(TjommieSpeechPlugin.supportedLanguage("en-GB", Arrays.asList("en-US")));
    assertFalse(TjommieSpeechPlugin.supportedLanguage("af-ZA", Arrays.asList("af-ZA")));
    assertFalse(TjommieSpeechPlugin.supportedLanguage(null, Arrays.asList("en-US")));
  }
}
