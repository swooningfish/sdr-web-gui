/** `use:indeterminate={bool}`: the "some selected" state of a select-all checkbox. */
export function indeterminate(node: HTMLInputElement, on: boolean) {
  node.indeterminate = on;
  return {
    update(v: boolean) {
      node.indeterminate = v;
    },
  };
}
