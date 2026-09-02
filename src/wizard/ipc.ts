/** Normalize Tauri `invoke` rejections and thrown values into a string. */
export function invokeError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  if (err && typeof err === "object" && "message" in err) {
    const message = (err as { message: unknown }).message;
    if (typeof message === "string" && message.length > 0) return message;
  }
  return String(err);
}

export const USERNAME_RE = /^[a-z_][a-z0-9_-]{0,31}$/;
export const HOSTNAME_RE = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;
export const MIN_PASSWORD = 6;
const RESERVED_USERNAMES = new Set([
  "root", "bin", "daemon", "mail", "ftp", "http", "nobody", "dbus", "git",
  "systemd-coredump", "systemd-network", "systemd-oom", "systemd-journal-remote",
  "systemd-resolve", "systemd-timesync", "tss", "uuidd", "alpm", "avahi", "cups",
  "cups-browsed", "lp", "_talkd", "polkitd", "rtkit", "qemu", "brltty", "gluster",
  "rpc", "libvirt-qemu", "pcscd", "nvidia-persistenced", "sddm",
]);

export function usernameError(value: string): string | null {
  const v = value.trim();
  if (!v) return "username is required";
  if (RESERVED_USERNAMES.has(v)) return "username is reserved by the system";
  if (!USERNAME_RE.test(v)) {
    return "lowercase letters, digits, _ or -; must start with a letter or _";
  }
  return null;
}

export function fullNameError(value: string | null): string | null {
  const v = value?.trim() ?? "";
  if (!v) return "Git author name is required";
  if (v.length > 100) return "Git author name must be 100 characters or fewer";
  if (/\p{Cc}/u.test(v)) return "Git author name contains unsupported characters";
  return null;
}

export function emailError(value: string | null): string | null {
  const v = value?.trim() ?? "";
  if (!v) return null;
  if (v.length > 254) return "email address is too long";
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v)) return "enter a valid email address";
  return null;
}

export function hostnameError(value: string): string | null {
  const v = value.trim();
  if (!v) return "hostname is required";
  if (!HOSTNAME_RE.test(v)) {
    return "hostname must be a DNS label (lowercase letters, digits, hyphens)";
  }
  return null;
}

export function passwordError(value: string, confirm: string): string | null {
  if (!value) return "password is required";
  if (value.length < MIN_PASSWORD) return `password must be at least ${MIN_PASSWORD} characters`;
  if (confirm.length > 0 && value !== confirm) return "passwords do not match";
  return null;
}

export function identityOk(username: string, hostname: string, password: string, confirm: string): boolean {
  return (
    usernameError(username) == null &&
    hostnameError(hostname) == null &&
    passwordError(password, confirm) == null &&
    password === confirm
  );
}

export type InstallStart = "prepare" | "stage" | "cidata" | "bootnext" | "done";

export function installStartFromJournal(step: string | undefined): InstallStart {
  switch (step) {
    case "bootNextSet":
      return "done";
    case "bootEntryCreated":
      return "bootnext";
    case "staged":
      return "cidata";
    case "cidataPartitionCreated":
      return "stage";
    default:
      return "prepare";
  }
}
