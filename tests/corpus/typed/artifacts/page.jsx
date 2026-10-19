// Handcrafted JSX source. Parse only; never execute.
const Widget = ({ children }) => <main>{children}</main>;
export function Page(props) {
  const endpoint = '/api/login';
  return <Widget><a href={endpoint} title="A &amp; B">Hello &lt; 世界 {props.message}</a></Widget>;
}
