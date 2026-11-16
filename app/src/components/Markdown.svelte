<script lang="ts">
  // Renders the safe tree from lib/markdown.ts as plain elements (no HTML string).
  import { parseMarkdown, type Block, type Inline } from '../lib/markdown';

  let { text }: { text: string } = $props();
  const blocks = $derived(parseMarkdown(text));

  let copied = $state<string | null>(null);
  async function copy(code: string) {
    try {
      await navigator.clipboard.writeText(code);
      copied = code;
      setTimeout(() => (copied = null), 1500);
    } catch {
      /* clipboard not available: the text can still be selected */
    }
  }
</script>

{#snippet inl(nodes: Inline[])}
  {#each nodes as n, k (k)}
    {#if n.t === 'text'}{n.text}{:else if n.t === 'code'}<code>{n.text}</code>{:else if n.t === 'strong'}<strong>{@render inl(n.kids)}</strong>{:else if n.t === 'em'}<em>{@render inl(n.kids)}</em>{:else if n.t === 'link'}<span class="mdlink">{@render inl(n.kids)}</span>{#if n.kids.length !== 1 || n.kids[0].t !== 'text' || n.kids[0].text !== n.href}<span class="mdurl"> ({n.href})</span>{/if}{/if}
  {/each}
{/snippet}

{#snippet blk(list: Block[])}
  {#each list as b, k (k)}
    {#if b.t === 'p'}
      <p>{@render inl(b.kids)}</p>
    {:else if b.t === 'h'}
      <p class="mdh mdh{Math.min(b.level, 3)}"><strong>{@render inl(b.kids)}</strong></p>
    {:else if b.t === 'code'}
      <div class="mdcode">
        <div class="mdcode-bar">
          <span class="muted small">{b.lang || 'code'}</span>
          <button class="ghost small" type="button" onclick={() => copy(b.text)} aria-label="Copy this code">{copied === b.text ? 'Copied' : 'Copy'}</button>
        </div>
        <pre><code>{b.text}</code></pre>
      </div>
    {:else if b.t === 'list'}
      {#if b.ordered}
        <ol start={b.start}>{#each b.items as it, j (j)}<li>{@render blk(it)}</li>{/each}</ol>
      {:else}
        <ul>{#each b.items as it, j (j)}<li>{@render blk(it)}</li>{/each}</ul>
      {/if}
    {:else if b.t === 'quote'}
      <blockquote>{@render blk(b.kids)}</blockquote>
    {:else if b.t === 'hr'}
      <hr />
    {:else if b.t === 'table'}
      <div class="mdtable">
        <table>
          <thead><tr>{#each b.head as c, j (j)}<th>{@render inl(c)}</th>{/each}</tr></thead>
          <tbody>
            {#each b.rows as r, j (j)}<tr>{#each r as c, m (m)}<td>{@render inl(c)}</td>{/each}</tr>{/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/each}
{/snippet}

<div class="md">{@render blk(blocks)}</div>
