import { VaultHostCommand, type VaultHostCommand as Command } from "../../protocol/src/vault.ts";

type Vault = { lock(): void; command(command: Command): Promise<unknown> };
type Options = { vault(): Vault | null; ready(): boolean; connection(): object | null; resolveBot(id: string): Promise<unknown> };

// Bot lookup can yield while a controller cancels or the protected host reconnects.
export class VaultCommandGate {
  readonly #options: Options;
  #epoch = 0;
  readonly #cancelled = new Map<string, number>();
  constructor(options: Options) { this.#options = options; }
  invalidate(): void { this.#epoch++; this.#options.vault()?.lock(); }
  async execute(raw: Command): Promise<unknown> {
    const vault = this.#options.vault();
    const command = VaultHostCommand.parse(raw);
    const cleanup = ["lock", "cancel", "disconnect"].includes(command.action.kind);
    if (!vault || !cleanup && !this.#options.ready()) throw new Error("Logins is unavailable. Check the host application.");
    if (command.action.kind === "cancel" || command.action.kind === "disconnect") this.#cancelled.set(command.controllerId, (this.#cancelled.get(command.controllerId) ?? 0) + 1);
    if (command.action.kind === "lock") this.#epoch++;
    const epoch = this.#epoch, cancelled = this.#cancelled.get(command.controllerId), connection = this.#options.connection();
    if ("botId" in command.action && command.action.botId) await this.#options.resolveBot(command.action.botId);
    if (!cleanup && !this.#options.ready() || epoch !== this.#epoch || cancelled !== this.#cancelled.get(command.controllerId) || connection !== this.#options.connection()) throw new Error("The Logins connection changed. Try again.");
    return vault.command(command);
  }
}
