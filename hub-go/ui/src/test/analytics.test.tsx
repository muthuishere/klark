import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { AuthProvider } from '../auth';
import { AdminAnalytics } from '../pages/Analytics';
import { Sparkline } from '../components/Sparkline';
import { fmtBytes } from '../lib/units';
import { mockFetch, setBase } from './helpers';

const wrap = (el: React.ReactNode) => render(<MemoryRouter><AuthProvider>{el}</AuthProvider></MemoryRouter>);
const series = [1, 2, 3].map((i) => ({ t: i * 1000, klarkRssBytes: i * 1048576, klarkCpuPercent: i, hostMemAvailableBytes: 2 ** 30, panes: 1, agents: 1 }));
const analytics = {
  range: '1h', intervalSeconds: 30, totals: { machines: 2, online: 1, panes: 3, agents: 2, klarkRssBytes: 3145728 },
  machines: [
    { id: 'a', name: 'alpha', online: true, hasStats: true, peak: { klarkRssBytes: 3145728, klarkCpuPercent: 3, hostMemUsedPercent: 40, panes: 2, agents: 2 }, series,
      latest: { ts: 1, klarkRssBytes: 3145728, klarkPeakRssBytes: 3145728, klarkCpuPercent: 3, uptimeSeconds: 3700, version: '1', hostMemTotalBytes: 2 ** 31, hostMemAvailableBytes: 2 ** 30, load1: 0.5, cpus: 4, panes: 2, agents: 2,
        agentList: [{ paneId: 'p1', name: 'small', state: 'idle', rssBytes: 1048576, cpuPercent: 1 }, { paneId: 'p2', name: 'big', state: 'working', rssBytes: 52428800, cpuPercent: 9 }] } },
    { id: 'b', name: 'beta', online: false, hasStats: false, latest: null },
  ],
};

describe('units and sparkline', () => {
  it('formats bytes', () => { expect(fmtBytes(1048576)).toBe('1.0 MiB'); expect(fmtBytes(3 * 2 ** 30)).toBe('3.0 GiB'); });
  it('renders values as text with a peak mark', () => {
    render(<Sparkline label="mem" points={[{ t: 1, v: 5 }, { t: 2, v: 9 }, { t: 3, v: 4 }]} format={(n) => `${n}u`} />);
    expect(screen.getByText('4u')).toBeInTheDocument();
    expect(screen.getByText('9u')).toBeInTheDocument();
    expect(screen.getByTestId('peak-mark')).toBeInTheDocument();
  });
});

const hub = {
  latest: { ts: 1, rssBytes: 20971520, peakRssBytes: 31457280, cpuPercent: 2.5, heapInuseBytes: 8388608, heapSysBytes: 16777216, sysBytes: 1, goroutines: 42, gcCount: 7,
    gcPauseTotalMs: 1, gcLastPauseMs: 0.1, uptimeSeconds: 3700, version: '1.2', goVersion: 'go1.25', os: 'linux/amd64', machineLinks: 3, sseStreams: 4, paneViewers: 1,
    requests: 100, requestsPerMinute: 12, dbBytes: 1048576, dbWalBytes: 0, assetsBytes: 2048, lastBackupMs: 0, lastAnalyticsWriteMs: 0 },
  peak: { rssBytes: 31457280, cpuPercent: 9, goroutines: 50, heapBytes: 8388608 },
  series: [1, 2].map((i) => ({ t: i, rssBytes: i * 1048576, cpuPercent: i, heapBytes: i, goroutines: 40 + i, streams: i })),
};

describe('analytics page', () => {
  beforeEach(() => setBase('/'));
  it('shows the hub card and a dash, not 0 B, when no machine reports', async () => {
    mockFetch((u) => {
      if (u.endsWith('api/auth/me')) return { body: { username: 'a', admin: true } };
      if (u.includes('api/admin/analytics')) return { body: { ...analytics, totals: { ...analytics.totals, klarkRssBytes: 0 }, machines: [analytics.machines[1]], hub } };
      return { body: {} };
    });
    wrap(<AdminAnalytics />);
    const card = await screen.findByRole('region', { name: 'Hub' });
    expect(card).toHaveTextContent('20.0 MiB');
    expect(card).toHaveTextContent('peak 30.0 MiB');
    expect(card).toHaveTextContent('42');
    expect(card).toHaveTextContent('1 terminal viewers');
    expect(screen.getByRole('img', { name: /goroutines: now 42/ })).toBeInTheDocument();
    expect(screen.getByText('no machine reports stats')).toBeInTheDocument();
    expect(screen.queryByText('0 B')).not.toBeInTheDocument();
  });

  beforeEach(() => setBase('/'));
  it('shows tiles, sorted agents, no-stats machines, changes range ', async () => {
    const calls = mockFetch((u) => {
      if (u.endsWith('api/auth/me')) return { body: { username: 'a', admin: true } };
      if (u.includes('api/admin/analytics')) return { body: analytics };
      return { body: {} };
    });
    wrap(<AdminAnalytics />);
    expect(await screen.findByText('1 / 2')).toBeInTheDocument();
    expect(screen.getByText(/no stats from this machine/)).toBeInTheDocument();
    const rows = screen.getAllByRole('row').map((r) => r.textContent);
    expect(rows[1]).toMatch(/big/); expect(rows[2]).toMatch(/small/);
    await userEvent.click(screen.getByRole('button', { name: '7d' }));
    await waitFor(() => expect(calls.some((c) => c.url.includes('range=7d'))).toBe(true));
  });
});

import { AdminStorage, duckdbExample } from '../pages/Storage';
describe('storage page', () => {
  beforeEach(() => setBase('/'));
  const info = { kind: 's3', source: 'admin', path: 'klark/prod', s3: { bucket: 'bk', region: 'eu', accessKeyId: 'AK', secretAccessKeySet: true, pathStyle: true },
    sqlite: { enabled: true, every: '6h0m0s', keep: 7, backups: 1 }, lastSqliteBackupMs: 0, lastAnalyticsWriteMs: 1700000000000, lastError: 'denied', filesWritten: 3 };
  it('never prefills the secret, keeps it when empty, tests and shows status', async () => {
    const calls = mockFetch((u, init) => {
      if (u.endsWith('api/auth/me')) return { body: { username: 'a', admin: true } };
      if (u.endsWith('storage/test')) return { body: { ok: true, message: 'wrote marker' } };
      return { body: info };
    });
    wrap(<AdminStorage />);
    expect(await screen.findByTestId('secret-set')).toBeInTheDocument();
    expect(screen.getByLabelText(/Secret access key/)).toHaveValue('');
    expect(screen.getByTestId('last-sqlite')).toHaveTextContent('never');
    expect(screen.getByRole('alert')).toHaveTextContent('denied');
    expect(screen.getByText(/s3:\/\/bk\/klark\/prod\/analytics\/\*\*\/\*\.ndjson\.gz/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(calls.some((c) => c.init.method === 'PUT')).toBe(true));
    const put = JSON.parse(String(calls.find((c) => c.init.method === 'PUT')!.init.body));
    expect(put).toEqual({ kind: 's3', path: 'klark/prod', s3: { bucket: 'bk', region: 'eu', accessKeyId: 'AK', pathStyle: true } });
    await userEvent.type(screen.getByLabelText(/Secret access key/), 'sek');
    await userEvent.click(screen.getByRole('button', { name: 'Test' }));
    expect(await screen.findByText(/wrote marker/)).toBeInTheDocument();
    expect(JSON.parse(String(calls.find((c) => c.url.endsWith('storage/test'))!.init.body)).s3.secretAccessKey).toBe('sek');
  });
  it('builds the DuckDB path for a folder', () => {
    expect(duckdbExample({ kind: 'folder', path: '/data/store/' })).toBe("SELECT * FROM read_json_auto('/data/store/analytics/**/*.ndjson.gz');");
  });
});
