import { BrowserRouter, Route, Routes } from 'react-router-dom';
import { AuthProvider, RequireAuth } from './auth';
import { routerBasename } from './lib/base';
import { Shell } from './shell';
import { Home } from './pages/Home';
import { Login } from './pages/Login';
import { MachinePage } from './pages/MachinePage';
import { PanePage } from './pages/PanePage';
import { FileViewer } from './pages/FileViewer';
import { Messages } from './pages/Messages';
import { AdminDirectory, AdminMachines, AdminUsers } from './pages/Admin';
import { Go } from './pages/Go';
import { NotFound } from './pages/NotFound';

export function AppRoutes() {
  return (
    <Routes>
      <Route path="/login" element={<Login />} />
      <Route element={<RequireAuth><Shell /></RequireAuth>}>
        <Route path="/" element={<Home />} />
        <Route path="/go" element={<Go />} />
        <Route path="/m/:machine" element={<MachinePage />} />
        <Route path="/m/:machine/p/:pane" element={<PanePage />} />
        <Route path="/m/:machine/f" element={<FileViewer />} />
        <Route path="/messages" element={<Messages />} />
        <Route path="/admin/users" element={<RequireAuth admin><AdminUsers /></RequireAuth>} />
        <Route path="/admin/machines" element={<RequireAuth admin><AdminMachines /></RequireAuth>} />
        <Route path="/admin/directory" element={<RequireAuth admin><AdminDirectory /></RequireAuth>} />
      </Route>
      <Route path="*" element={<NotFound />} />
    </Routes>
  );
}

export default function App() {
  return (
    <BrowserRouter basename={routerBasename()} future={{ v7_startTransition: true, v7_relativeSplatPath: true }}>
      <AuthProvider><AppRoutes /></AuthProvider>
    </BrowserRouter>
  );
}
