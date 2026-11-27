import { describe, expect, it } from "vitest";
import {
  buildSafeCdCommand,
  buildSafeInsertPath,
  containsControlChars,
  isDangerousMultilinePaste,
  parseHostKeyError,
} from "./security";

describe("security utilities", () => {
  it("parses structured HOST_KEY_UNKNOWN errors", () => {
    const err =
      'HOST_KEY_UNKNOWN::{"hostPort":"192.168.9.165:22","algorithm":"ssh-ed25519","fingerprint":"SHA256:abc123","isJumpHost":false}';
    const parsed = parseHostKeyError(err);
    expect(parsed).toEqual({
      kind: "unknown",
      info: {
        hostPort: "192.168.9.165:22",
        algorithm: "ssh-ed25519",
        fingerprint: "SHA256:abc123",
        isJumpHost: false,
      },
    });
  });

  it("parses structured HOST_KEY_MISMATCH errors for jump hosts", () => {
    const err =
      'HOST_KEY_MISMATCH::{"hostPort":"bastion:22","expectedAlgorithm":"ssh-ed25519","expectedFingerprint":"SHA256:old","actualAlgorithm":"ssh-ed25519","actualFingerprint":"SHA256:evil","isJumpHost":true}';
    const parsed = parseHostKeyError(err);
    expect(parsed?.kind).toBe("mismatch");
    if (parsed?.kind === "mismatch") {
      expect(parsed.info.isJumpHost).toBe(true);
      expect(parsed.info.expectedFingerprint).toBe("SHA256:old");
      expect(parsed.info.actualFingerprint).toBe("SHA256:evil");
    }
  });

  it("ignores normal error strings", () => {
    expect(parseHostKeyError("认证失败: 密码错误")).toBeNull();
    expect(parseHostKeyError("HOST_KEY_UNKNOWN::{broken-json")).toBeNull();
  });

  it("builds safe POSIX and Windows cd commands and blocks injection", () => {
    expect(buildSafeCdCommand("/var/www/my app")).toBe("cd '/var/www/my app'\r");
    expect(buildSafeCdCommand("/var/o'hara")).toBe("cd '/var/o'\\''hara'\r");
    expect(buildSafeCdCommand("/D:/Projects/demo")).toBe('cd "D:\\Projects\\demo"\r');

    // 必须拦截换行符、NUL 及 Windows 危险命令元字符
    expect(buildSafeCdCommand("/tmp/a\nrm -rf /")).toBeNull();
    expect(buildSafeCdCommand("/tmp/a\rwhoami")).toBeNull();
    expect(buildSafeCdCommand("/tmp/a\0b")).toBeNull();
    expect(buildSafeCdCommand('/C:/foo" & calc & "')).toBeNull();
    expect(buildSafeCdCommand("/C:/foo$(whoami)")).toBeNull();
    expect(buildSafeCdCommand("/C:/foo`id`")).toBeNull();
  });

  it("builds safe insert paths and rejects control characters", () => {
    expect(buildSafeInsertPath("/var/log/syslog")).toBe("/var/log/syslog ");
    expect(buildSafeInsertPath("/var/log/my file.log")).toBe("'/var/log/my file.log' ");
    expect(buildSafeInsertPath("/tmp/evil\nreboot")).toBeNull();
    expect(containsControlChars("normal-text")).toBe(false);
    expect(containsControlChars("escape\x1b[31m")).toBe(true);
  });

  it("detects dangerous multiline clipboard pastes", () => {
    expect(isDangerousMultilinePaste("ls -la")).toBe(false);
    expect(isDangerousMultilinePaste("ls -la\nrm -rf /")).toBe(true);
    expect(isDangerousMultilinePaste("echo hi\r\nwhoami")).toBe(true);
  });
});
