// TS fixture: exercises the arrow-function-in-const pattern that aider's
// default query missed. If the predicate-stripped tingle query regresses,
// this file's captures will change.

const getInputLines = async (fileName: string): Promise<string[]> => {
  return fileName.split("\n");
};

const getParagraphs = async (fileName: string) => {
  return fileName.split("\n\n");
};

export async function readFile(path: string): Promise<string[]> {
  return [path];
}

export interface AuthService {
  login(user: string, pass: string): Promise<Session>;
  logout(): void;
}

export type Session = {
  user: string;
  token: string;
};

export class AuthServiceImpl implements AuthService {
  constructor(private key: string) {}

  async login(user: string, pass: string): Promise<Session> {
    return { user, token: this.key };
  }

  logout(): void {}
}

export { getInputLines, getParagraphs };
