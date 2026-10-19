// Handcrafted typed browser UI source. Parse only; never execute.
import type { ReactNode } from 'react';
interface Props { user: { name: string }; onSubmit: (name: string) => void; }
type Result<T> = { value: T; error?: string };
const Banner = ({ children }: { children: ReactNode }) => <section>{children}</section>;
export function Login<T extends Props>(props: T): ReactNode {
  const endpoint: string = '/api/login';
  const submit = () => props.onSubmit(props.user.name);
  return <Banner><button aria-label="Sign &amp; in" data-endpoint={endpoint} onClick={submit}>欢迎 {props.user.name} &#x1F600;</button></Banner>;
}
export const initial: Result<string> = { value: 'ready' };
