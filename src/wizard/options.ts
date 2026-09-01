export type PickerOption = {
  id: string;
  label: string;
};

// Kept in sync with OMARCHY_KEYBOARD_LAYOUTS in Omarchy's setup-form.sh.
export const KEYBOARDS: PickerOption[] = [
  { id: "us", label: "English (US)" },
  { id: "uk", label: "English (UK)" },
  { id: "dvorak", label: "English (US, Dvorak)" },
  { id: "colemak", label: "English (US, Colemak)" },
  { id: "azerty", label: "Azerbaijani" },
  { id: "by", label: "Belarusian" },
  { id: "be-latin1", label: "Belgian" },
  { id: "bg-cp1251", label: "Bulgarian" },
  { id: "croat", label: "Croatian" },
  { id: "cz", label: "Czech" },
  { id: "dk-latin1", label: "Danish" },
  { id: "nl", label: "Dutch" },
  { id: "et", label: "Estonian" },
  { id: "fi", label: "Finnish" },
  { id: "fr", label: "French" },
  { id: "cf", label: "French (Canada)" },
  { id: "fr_CH", label: "French (Switzerland)" },
  { id: "ge", label: "Georgian" },
  { id: "de", label: "German" },
  { id: "de_CH-latin1", label: "German (Switzerland)" },
  { id: "gr", label: "Greek" },
  { id: "il", label: "Hebrew" },
  { id: "hu", label: "Hungarian" },
  { id: "is-latin1", label: "Icelandic" },
  { id: "ie", label: "Irish" },
  { id: "it", label: "Italian" },
  { id: "jp106", label: "Japanese" },
  { id: "kazakh", label: "Kazakh" },
  { id: "kyrgyz", label: "Kyrgyz" },
  { id: "la-latin1", label: "Lao" },
  { id: "lv", label: "Latvian" },
  { id: "lt", label: "Lithuanian" },
  { id: "mk-utf", label: "Macedonian" },
  { id: "no-latin1", label: "Norwegian" },
  { id: "pl", label: "Polish" },
  { id: "pt-latin1", label: "Portuguese" },
  { id: "br-abnt2", label: "Portuguese (Brazil)" },
  { id: "ro", label: "Romanian" },
  { id: "ru", label: "Russian" },
  { id: "sr-latin", label: "Serbian" },
  { id: "sk-qwertz", label: "Slovak" },
  { id: "slovene", label: "Slovenian" },
  { id: "es", label: "Spanish" },
  { id: "la-latin1", label: "Spanish (Latin American)" },
  { id: "sv-latin1", label: "Swedish" },
  { id: "tj_alt-UTF8", label: "Tajik" },
  { id: "trq", label: "Turkish" },
  { id: "ua", label: "Ukrainian" },
];

function supportedTimezones(): string[] {
  const detected = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const intl = Intl as typeof Intl & {
    supportedValuesOf?: (key: "timeZone") => string[];
  };
  const supported = intl.supportedValuesOf?.("timeZone") ?? [];
  return Array.from(new Set([detected, "UTC", ...supported].filter(Boolean))).sort((a, b) =>
    a.localeCompare(b),
  );
}

export const TIMEZONES = supportedTimezones();
export const TIMEZONE_OPTIONS: PickerOption[] = TIMEZONES.map((timezone) => ({
  id: timezone,
  label: timezone.replace(/_/g, " "),
}));
