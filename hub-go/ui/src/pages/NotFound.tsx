import { Link } from 'react-router-dom';

export function NotFound() {
  return (
    <main className="login">
      <h1>Not found</h1>
      <p className="empty">There is nothing at this address.</p>
      <Link to="/">Back to klark</Link>
    </main>
  );
}
