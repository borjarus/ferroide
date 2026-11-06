export interface Greeting {
  name: string;
  excited?: boolean;
}

export function greet({ name, excited = false }: Greeting): string {
  const base = name ? `Hello, ${name}` : "Hello, stranger";
  return excited ? `${base}!` : base;
}

document.querySelector("#app")!.textContent = greet({ name: "FerroIDE", excited: true });
