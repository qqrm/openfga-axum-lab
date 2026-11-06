"use strict";
const pets = [
  {id:"cat",name:"Pixel",species:"Кот",icon:"🐱",owner:"alice",viewers:["dave"],description:"Любит долгий сон и мурчит громче всех."},
  {id:"dog",name:"Rex",species:"Собака",icon:"🐶",owner:"bob",viewers:[],description:"Обожает прогулки и новых друзей."},
  {id:"fox",name:"Mika",species:"Лиса",icon:"🦊",owner:"carol",viewers:[],description:"Любопытная лиса с любовью к тишине."}
];
async function start() {
  const response = await fetch("domain.wasm");
  if (!response.ok) throw new Error("Не удалось загрузить доменную модель.");
  const {instance} = await WebAssembly.instantiate(await response.arrayBuffer());
  if(instance.exports.next_state(1,1)!==2 || instance.exports.is_visible(2)!==0) throw new Error("Проверка модели не прошла.");
  function draw() {
    const user=document.getElementById("user").value, cards=document.getElementById("cards");
    cards.replaceChildren();
    for (const pet of pets) {
      // Deliberate fixture simulation, not an OpenFGA decision.
      if (!instance.exports.is_visible(1) || !(user==="alice" || user==="bob" || user===pet.owner || pet.viewers.includes(user))) continue;
      const card=document.createElement("article"); card.className="card";
      const portrait=document.createElement("div"); portrait.className="portrait"; portrait.textContent=pet.icon;
      const content=document.createElement("div"); content.className="content";
      const name=document.createElement("h3"); name.textContent=pet.name;
      const description=document.createElement("p"); description.textContent=pet.description;
      const species=document.createElement("span"); species.className="chip"; species.textContent=pet.species;
      content.append(name,description,species); card.append(portrait,content); cards.append(card);
    }
  }
  document.getElementById("user").addEventListener("change",draw); draw();
}
start().catch(error=>{document.getElementById("error").textContent=error.message});
