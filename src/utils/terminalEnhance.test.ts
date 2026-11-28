import { describe, expect, it } from "vitest";
import {
  DEFAULT_HIGHLIGHT_RULES,
  highlightTerminalText,
  isSensitiveCommand,
  matchDangerousCommand,
  parseOsc52Write,
  parseOsc7Path,
} from "./terminalEnhance";

describe("terminalEnhance utilities", () => {
  it("detects dangerous commands and allows safe ones", () => {
    expect(matchDangerousCommand("rm -rf /")).not.toBeNull();
    expect(matchDangerousCommand("sudo rm -fr /*")).not.toBeNull();
    expect(matchDangerousCommand("rm -rf ~")).not.toBeNull();
    expect(matchDangerousCommand("sudo mkfs.ext4 /dev/sdb1")).not.toBeNull();
    expect(matchDangerousCommand("dd if=/dev/zero of=/dev/sda bs=1M")).not.toBeNull();
    expect(matchDangerousCommand("sudo reboot")).not.toBeNull();
    expect(matchDangerousCommand("chmod -R 777 /")).not.toBeNull();
    expect(matchDangerousCommand(":(){ :|:& };:")).not.toBeNull();
    expect(matchDangerousCommand("DROP DATABASE prod;")).not.toBeNull();

    // 正常日常命令不应误报
    expect(matchDangerousCommand("rm -rf ./dist")).toBeNull();
    expect(matchDangerousCommand("ls -la /var/log")).toBeNull();
    expect(matchDangerousCommand("docker compose up -d")).toBeNull();
    expect(matchDangerousCommand("nvim ~/.zshrc")).toBeNull();
  });

  it("filters sensitive commands from history recording", () => {
    expect(isSensitiveCommand("passwd root")).toBe(true);
    expect(isSensitiveCommand("export API_KEY=sk-12345")).toBe(true);
    expect(isSensitiveCommand("mysql -u root -pMySecret")).toBe(true);
    expect(isSensitiveCommand("curl -H 'Authorization: Bearer xxx' https://a")).toBe(true);

    expect(isSensitiveCommand("docker ps -a")).toBe(false);
    expect(isSensitiveCommand("journalctl -u nginx -n 50")).toBe(false);
  });

  it("parses valid OSC 7 file URIs and rejects control chars", () => {
    expect(parseOsc7Path("file://debian13/var/www/html/")).toBe("/var/www/html");
    expect(parseOsc7Path("file://host/home/user/my%20app")).toBe("/home/user/my app");
    expect(parseOsc7Path("file://host/")).toBe("/");
    expect(parseOsc7Path("http://example.com/a")).toBeNull();
    expect(parseOsc7Path("file://host/tmp/a\nb")).toBeNull();
  });

  it("highlights keywords without corrupting existing ANSI sequences", () => {
    const input = "\x1b[34m[service]\x1b[0m ERROR connecting to 192.168.9.165 (WARN retry)";
    const out = highlightTerminalText(input, DEFAULT_HIGHLIGHT_RULES);
    expect(out).toContain("\x1b[34m[service]\x1b[0m");
    expect(out).toContain("\x1b[1;31mERROR\x1b[0m");
    expect(out).toContain("\x1b[36m192.168.9.165\x1b[0m");
    expect(out).toContain("\x1b[1;33mWARN\x1b[0m");
  });

  it("allows UTF-8 OSC 52 write while strictly blocking OSC 52 clipboard read", () => {
    // 严格阻断任何远端读取本地剪贴板的 `?` 请求
    expect(parseOsc52Write("c;?")).toBeNull();
    expect(parseOsc52Write(";?")).toBeNull();
    expect(parseOsc52Write("p;?")).toBeNull();
    expect(parseOsc52Write("c;")).toBeNull();

    // 正常放行 tmux / nvim 的 UTF-8 Base64 写入（"你好 tmux"）
    const bytes = new TextEncoder().encode("你好 tmux");
    let bin = "";
    for (const b of bytes) bin += String.fromCharCode(b);
    expect(parseOsc52Write(`c;${btoa(bin)}`)).toBe("你好 tmux");
  });
});
