import { describe, expect, it } from "vitest";
import {
  buildAsciinemaV2,
  buildOsCdCommand,
  DEFAULT_HIGHLIGHT_RULES,
  detectTerminalOs,
  encodeTerminalPaste,
  formatBatchScriptForOs,
  getBatchPresets,
  hasIncompleteAnsiTail,
  highlightTerminalText,
  isPasswordPromptLine,
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

  it("filters sensitive commands and detects password prompt lines", () => {
    expect(isSensitiveCommand("passwd root")).toBe(true);
    expect(isSensitiveCommand("export API_KEY=sk-12345")).toBe(true);
    expect(isSensitiveCommand("mysql -u root -pMySecret")).toBe(true);
    expect(isSensitiveCommand("curl -H 'Authorization: Bearer xxx' https://a")).toBe(true);

    expect(isSensitiveCommand("docker ps -a")).toBe(false);
    expect(isSensitiveCommand("journalctl -u nginx -n 50")).toBe(false);

    // 密码/口令提示行识别
    expect(isPasswordPromptLine("[sudo] password for hailan20251015: ")).toBe(true);
    expect(isPasswordPromptLine("Enter passphrase for key '/home/u/.ssh/id_ed25519':")).toBe(
      true
    );
    expect(isPasswordPromptLine("请输入 sudo 密码：")).toBe(true);
    expect(isPasswordPromptLine("user@debian13:~/app$ ls -la")).toBe(false);
  });

  it("encodes paste payloads safely for bracketed and non-bracketed paste modes", () => {
    // 非括号粘贴模式：统一 CRLF/LF -> CR
    expect(encodeTerminalPaste("echo 1\r\necho 2\n", false)).toBe("echo 1\recho 2\r");

    // 括号粘贴模式：包裹 \x1b[200~...\x1b[201~ 并剥离恶意注入的逃逸序列
    expect(encodeTerminalPaste("line1\r\nline2", true)).toBe("\x1b[200~line1\rline2\x1b[201~");
    expect(encodeTerminalPaste("ok\x1b[201~rm -rf /\x1b[200~", true)).toBe(
      "\x1b[200~okrm -rf /\x1b[201~"
    );
  });

  it("parses valid OSC 7 file URIs and rejects control chars", () => {
    expect(parseOsc7Path("file://debian13/var/www/html/")).toBe("/var/www/html");
    expect(parseOsc7Path("file://host/home/user/my%20app")).toBe("/home/user/my app");
    expect(parseOsc7Path("file://host/")).toBe("/");
    expect(parseOsc7Path("http://example.com/a")).toBeNull();
    expect(parseOsc7Path("file://host/tmp/a\nb")).toBeNull();
  });

  it("highlights keywords without wiping background colors or corrupting split ANSI", () => {
    const input = "\x1b[44m[service]\x1b[0m ERROR connecting to 192.168.9.165 (WARN retry)";
    const out = highlightTerminalText(input, DEFAULT_HIGHLIGHT_RULES);
    expect(out).toContain("\x1b[44m[service]\x1b[0m");
    expect(out).toContain("\x1b[1;31mERROR\x1b[39;22m");
    expect(out).toContain("\x1b[36m192.168.9.165\x1b[39;22m");
    expect(out).toContain("\x1b[1;33mWARN\x1b[39;22m");

    // 跨包截断的残缺 ANSI 转义序列必须原样直出
    const splitChunk = "ERROR \x1b[38;2;255;";
    expect(hasIncompleteAnsiTail(splitChunk)).toBe(true);
    expect(highlightTerminalText(splitChunk, DEFAULT_HIGHLIGHT_RULES)).toBe(splitChunk);
    expect(hasIncompleteAnsiTail(input)).toBe(false);
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

  it("differentiates Linux, Windows PowerShell, and Windows CMD commands and builds asciinema v2", () => {
    expect(detectTerminalOs("ssh", "linux")).toBe("linux");
    expect(detectTerminalOs("ssh", "windows")).toBe("windows_ps");
    expect(detectTerminalOs("local", undefined, "cmd")).toBe("windows_cmd");
    expect(detectTerminalOs("local", undefined, "wsl")).toBe("linux");
    expect(detectTerminalOs("local", undefined, "default")).toBe("windows_ps");

    // 不同操作系统的批执行预设完全隔离
    const linuxCmds = getBatchPresets("linux").map((p) => p.cmd).join(" ");
    const winPsCmds = getBatchPresets("windows_ps").map((p) => p.cmd).join(" ");
    const winCmds = getBatchPresets("windows_cmd").map((p) => p.cmd).join(" ");
    expect(linuxCmds).toContain("df -hT");
    expect(winPsCmds).toContain("Get-PSDrive");
    expect(winCmds).toContain("netstat -ano");

    // 按系统生成对应的目录切换命令
    expect(buildOsCdCommand("/var/www/app", "linux")).toBe("cd '/var/www/app'\r");
    expect(buildOsCdCommand("/D:/work/demo", "windows_ps")).toBe(
      "Set-Location -LiteralPath 'D:\\work\\demo'\r"
    );
    expect(buildOsCdCommand("/D:/work/demo", "windows_cmd")).toBe('cd /d "D:\\work\\demo"\r');
    expect(buildOsCdCommand("/tmp\nrm -rf /", "linux")).toBeNull();

    // 批执行脚本规范化跳过注释行
    expect(formatBatchScriptForOs("# comment\nuptime\nfree -h\n", "linux")).toBe(
      "uptime\rfree -h\r"
    );
    expect(formatBatchScriptForOs("REM skip\ndir\n", "windows_cmd")).toBe("dir\r");

    // 标准 asciinema v2 (.cast) 录制格式
    const cast = buildAsciinemaV2(120, 32, "test", [{ time: 0.25, text: "hello\r\n" }]);
    const lines = cast.trim().split("\n");
    expect(lines.length).toBe(2);
    expect(JSON.parse(lines[0]).version).toBe(2);
    expect(JSON.parse(lines[1])).toEqual([0.25, "o", "hello\r\n"]);
  });
});
