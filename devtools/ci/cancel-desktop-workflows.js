'use strict';

const DESKTOP_WORKFLOWS = new Set([
  'ci_benchmarks_macos',
  'ci_benchmarks_windows',
  'ci_integration_tests_macos',
  'ci_integration_tests_windows',
  'ci_linters_macos',
  'ci_quick_checks_macos',
  'ci_quick_checks_windows',
  'ci_unit_tests_macos',
  'ci_unit_tests_windows',
]);

const RUN_STATUSES = ['queued', 'in_progress', 'waiting'];

function requiredEnv(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

async function githubRequest(url, options = {}) {
  const token = requiredEnv('GITHUB_TOKEN');
  const response = await fetch(url, {
    ...options,
    headers: {
      accept: 'application/vnd.github+json',
      authorization: `Bearer ${token}`,
      'x-github-api-version': '2022-11-28',
      ...(options.headers || {}),
    },
  });

  if (!response.ok) {
    const body = await response.text();
    const error = new Error(`${options.method || 'GET'} ${url} failed: ${response.status} ${body}`);
    error.status = response.status;
    throw error;
  }

  const body = await response.text();
  if (!body) {
    return null;
  }
  return JSON.parse(body);
}

async function listWorkflowRuns({ apiBase, owner, repo, headSha, eventName, status }) {
  const runs = [];
  for (let page = 1; ; page += 1) {
    const params = new URLSearchParams({
      head_sha: headSha,
      status,
      per_page: '100',
      page: String(page),
    });
    if (eventName) {
      params.set('event', eventName);
    }

    const data = await githubRequest(`${apiBase}/repos/${owner}/${repo}/actions/runs?${params}`);
    runs.push(...data.workflow_runs);
    if (data.workflow_runs.length < 100) {
      return runs;
    }
  }
}

async function main() {
  const repository = requiredEnv('GITHUB_REPOSITORY');
  const [owner, repo] = repository.split('/');
  const headSha = requiredEnv('FAILED_HEAD_SHA');
  const eventName = process.env.FAILED_EVENT_NAME || '';
  const failedWorkflowName = process.env.FAILED_WORKFLOW_NAME || 'manual dispatch';
  const failedRunId = process.env.FAILED_RUN_ID || '';
  const currentRunId = process.env.GITHUB_RUN_ID || '';
  const apiBase = process.env.GITHUB_API_URL || 'https://api.github.com';

  const runsById = new Map();
  for (const status of RUN_STATUSES) {
    const runs = await listWorkflowRuns({ apiBase, owner, repo, headSha, eventName, status });
    for (const run of runs) {
      if (String(run.id) === failedRunId || String(run.id) === currentRunId) {
        continue;
      }
      if (!DESKTOP_WORKFLOWS.has(run.name)) {
        continue;
      }
      runsById.set(run.id, run);
    }
  }

  if (runsById.size === 0) {
    console.log(`No running desktop CI workflows found for ${headSha}.`);
    return;
  }

  console.log(`Cancelling desktop CI after ${failedWorkflowName} failed for ${headSha}.`);
  for (const run of runsById.values()) {
    try {
      console.log(`Cancelling ${run.name} #${run.run_number}: ${run.html_url}`);
      await githubRequest(`${apiBase}/repos/${owner}/${repo}/actions/runs/${run.id}/cancel`, {
        method: 'POST',
      });
    } catch (error) {
      if (error.status === 404 || error.status === 409) {
        console.warn(`Could not cancel ${run.name} #${run.run_number}: ${error.message}`);
        continue;
      }
      throw error;
    }
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
